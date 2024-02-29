use std::{
    str,
    task::{Context, Poll},
};

use futures_core::Stream;
use futures_util::StreamExt;

use crate::{error::MultipartError, multipart_type::MultipartType};

#[derive(PartialEq, Debug)]
enum InnerState {
    /// Stream eof
    Eof,

    /// Skip data until first boundary
    FirstBoundary,

    /// Reading boundary
    Boundary,

    /// Reading Headers,
    Headers,
}

pub struct MultipartItem {
    /// Headers
    headers: Vec<(String, String)>,

    /// Data
    data: Vec<u8>,
}

pub struct MultipartReader<'a> {
    /// Inner state
    pub boundary: String,
    data: &'a [u8],
    state: InnerState,
    pub multipart_type: MultipartType,
    pending_item: Option<MultipartItem>,
}

impl<'a> MultipartReader<'a> {
    pub fn from_data_with_boundary_and_type(
        data: &'a [u8],
        boundary: &str,
        multipart_type: MultipartType,
    ) -> Result<MultipartReader<'a>, MultipartError> {
        Ok(MultipartReader {
            data: data,
            boundary: boundary.to_string(),
            multipart_type: multipart_type,
            state: InnerState::FirstBoundary,
            pending_item: None,
        })
    }

    pub fn from_data_with_headers(
        data: &'a [u8],
        headers: &Vec<(String, String)>,
    ) -> Result<MultipartReader<'a>, MultipartError> {
        // Search for the content-type header
        let content_type = headers
            .iter()
            .find(|(key, _)| key.to_lowercase() == "content-type");

        if content_type.is_none() {
            return Err(MultipartError::NoContentType);
        }

        let ct = content_type
            .unwrap()
            .1
            .parse::<mime::Mime>()
            .map_err(|_e| MultipartError::InvalidContentType)?;
        let boundary = ct
            .get_param(mime::BOUNDARY)
            .ok_or(MultipartError::InvalidBoundary)?;

        if ct.type_() != mime::MULTIPART {
            return Err(MultipartError::InvalidContentType);
        }

        let multipart_type = ct
            .subtype()
            .as_str()
            .parse::<MultipartType>()
            .map_err(|_| MultipartError::InvalidMultipartType)?;

        Ok(MultipartReader {
            data: data,
            boundary: boundary.to_string(),
            multipart_type: multipart_type,
            state: InnerState::FirstBoundary,
            pending_item: None,
        })
    }

    // TODO: make this RFC compliant
    fn is_boundary(self: &Self, data: &[u8]) -> bool {
        data.starts_with(self.boundary.as_bytes())
    }
}

impl<'a> Stream for MultipartReader<'a> {
    type Item = Result<MultipartItem, MultipartError>;

    fn poll_next(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut Context<'_>,
    ) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        let finder = memchr::memmem::Finder::new("\r\n");

        while let Some(idx) = finder.find(this.data) {
            println!("{}", String::from_utf8_lossy(&this.data[..idx]));
            match this.state {
                InnerState::FirstBoundary => {
                    // Check if the last line was a boundary
                    if this.is_boundary(&this.data[..idx]) {
                        this.state = InnerState::Headers;
                    };
                }
                InnerState::Boundary => {
                    // Check if the last line was a boundary
                    if this.is_boundary(&this.data[..idx]) {
                        // If we have a pending item, return it
                        if let Some(item) = this.pending_item.take() {
                            // Skip to the next line
                            this.data = &this.data[2 + idx..];
                            // Next state are the headers
                            this.state = InnerState::Headers;
                            return std::task::Poll::Ready(Some(Ok(item)));
                        }

                        this.state = InnerState::Headers;
                        this.pending_item = Some(MultipartItem {
                            headers: vec![],
                            data: vec![],
                        });
                    };

                    // Add the data to the pending item
                    this.pending_item
                        .as_mut()
                        .unwrap()
                        .data
                        .extend_from_slice(&this.data[..idx]);
                }
                InnerState::Headers => {
                    // Check if we have a pending item or we should create one
                    if this.pending_item.is_none() {
                        this.pending_item = Some(MultipartItem {
                            headers: vec![],
                            data: vec![],
                        });
                    }

                    // Read the header line and split it into key and value
                    let header = match str::from_utf8(&this.data[..idx]) {
                        Ok(h) => h,
                        Err(_) => {
                            this.state = InnerState::Eof;
                            return std::task::Poll::Ready(Some(Err(
                                MultipartError::InvalidItemHeader,
                            )));
                        }
                    };

                    // This is no header anymore, we are at the end of the headers
                    if header.trim().is_empty() {
                        this.data = &this.data[2 + idx..];
                        this.state = InnerState::Boundary;
                        continue;
                    }

                    let header_parts: Vec<&str> = header.split(": ").collect();
                    if header_parts.len() != 2 {
                        this.state = InnerState::Eof;
                        return std::task::Poll::Ready(Some(Err(
                            MultipartError::InvalidItemHeader,
                        )));
                    }

                    // Add header entry to the pending item
                    this.pending_item
                        .as_mut()
                        .unwrap()
                        .headers
                        .push((header_parts[0].to_string(), header_parts[1].to_string()));
                }
                InnerState::Eof => {
                    return std::task::Poll::Ready(None);
                }
            }

            // Skip to the next line
            this.data = &this.data[2 + idx..];
        }

        std::task::Poll::Ready(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[futures_test::test]
    async fn valid_request() {
        let headermap = vec![(
            "Content-Type".to_string(),
            "multipart/form-data; boundary=--974767299852498929531610575".to_string(),
        )];
        // Lines must end with CRLF
        let data = b"--974767299852498929531610575\r
Content-Disposition: form-data; name=\"text\"\r
\r
text default\r
--974767299852498929531610575\r
Content-Disposition: form-data; name=\"file1\"; filename=\"a.txt\"\r
Content-Type: text/plain\r
\r
Content of a.txt.\r
\r\n--974767299852498929531610575\r
Content-Disposition: form-data; name=\"file2\"; filename=\"a.html\"\r
Content-Type: text/html\r
\r
<!DOCTYPE html><title>Content of a.html.</title>\r
\r
--974767299852498929531610575--\r\n";

        assert!(MultipartReader::from_data_with_headers(data, &headermap).is_ok());
        assert!(MultipartReader::from_data_with_boundary_and_type(
            data,
            "--974767299852498929531610575",
            MultipartType::FormData
        )
        .is_ok());

        // Poll all the items from the reader
        let mut reader = MultipartReader::from_data_with_headers(data, &headermap).unwrap();
        assert_eq!(reader.multipart_type, MultipartType::FormData);
        let mut items = vec![];

        loop {
            match reader.next().await {
                Some(Ok(item)) => items.push(item),
                None => break,
                Some(Err(e)) => panic!("Error: {:?}", e),
            }
        }

        assert_eq!(items.len(), 3);
    }
}
