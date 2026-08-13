use std::io::{self, Read, Write};

use bytes::Bytes;
use futures_core::Stream;
use futures_util::{future, stream, StreamExt};
use uuid::Uuid;

pub struct MultipartWriter {
    pub boundary: String,
    pub data: Vec<u8>,
    first: bool,
}

impl Default for MultipartWriter {
    fn default() -> Self {
        Self::new()
    }
}

impl MultipartWriter {
    pub fn new() -> MultipartWriter {
        MultipartWriter {
            boundary: format!("{}", Uuid::new_v4()),
            first: true,
            data: Vec::new(),
        }
    }

    pub fn new_with_boundary(boundary: &str) -> MultipartWriter {
        MultipartWriter {
            boundary: boundary.to_string(),
            first: true,
            data: Vec::new(),
        }
    }

    pub fn add(&mut self, mut reader: impl Read, headers: &str) -> io::Result<u64> {
        // writer for our buffer
        let mut writer = std::io::BufWriter::new(&mut self.data);

        // write the boundary
        if !self.first {
            writer.write_all(b"\r\n")?;
        }

        // set first to false
        self.first = false;

        writer.write_all(b"--")?;
        writer.write_all(self.boundary.as_bytes())?;
        writer.write_all(b"\r\n")?;

        // write the content type
        writer.write_all(headers.as_bytes())?;

        // write an empty line
        writer.write_all(b"\r\n")?;
        writer.write_all(b"\r\n")?;

        // write the content
        io::copy(&mut reader, &mut writer)
    }

    pub fn finish(&mut self) -> io::Result<()> {
        // writer for our buffer
        let mut writer = std::io::BufWriter::new(&mut self.data);

        // write the final boundary
        writer.write_all(b"\r\n")?;
        writer.write_all(b"--")?;
        writer.write_all(self.boundary.as_bytes())?;
        writer.write_all(b"--")?;
        writer.write_all(b"\r\n")?;

        Ok(())
    }
}

/// A streaming counterpart to [`MultipartWriter`].
///
/// Instead of buffering the whole multipart document in memory, it turns a
/// stream of parts into a stream of byte chunks, so only the chunks currently
/// in flight are held in memory. The produced framing is byte-identical to
/// [`MultipartWriter`].
pub struct MultipartStreamWriter {
    pub boundary: String,
}

impl Default for MultipartStreamWriter {
    fn default() -> Self {
        Self::new()
    }
}

impl MultipartStreamWriter {
    pub fn new() -> MultipartStreamWriter {
        MultipartStreamWriter {
            boundary: format!("{}", Uuid::new_v4()),
        }
    }

    pub fn new_with_boundary(boundary: &str) -> MultipartStreamWriter {
        MultipartStreamWriter {
            boundary: boundary.to_string(),
        }
    }

    /// Turn a stream of parts into a stream of multipart byte chunks,
    /// including the closing boundary.
    ///
    /// Each part is a `(headers, data)` pair, where `headers` are the raw
    /// header lines without the terminating empty line (e.g.
    /// `"Content-Type: application/dicom"`) and `data` is the part's byte
    /// stream. Parts are emitted in order; a part is only polled once all
    /// previous parts have been fully emitted, so parts can be produced
    /// lazily. An `Err` item (from the part stream or a data stream) is
    /// forwarded and ends the multipart stream.
    pub fn stream<P, S, E>(self, parts: P) -> impl Stream<Item = Result<Bytes, E>>
    where
        P: Stream<Item = Result<(String, S), E>>,
        S: Stream<Item = Result<Bytes, E>>,
    {
        let boundary = self.boundary;
        let closing = Bytes::from(format!("\r\n--{}--\r\n", boundary));
        let mut first = true;
        parts
            .map(move |part| match part {
                Ok((headers, data)) => {
                    let prefix = if first { "" } else { "\r\n" };
                    first = false;
                    let head = Bytes::from(format!("{prefix}--{boundary}\r\n{headers}\r\n\r\n"));
                    stream::once(future::ready(Ok(head)))
                        .chain(data)
                        .left_stream()
                }
                Err(e) => stream::once(future::ready(Err(e))).right_stream(),
            })
            .flatten()
            .chain(stream::once(future::ready(Ok(closing))))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MultipartReader;
    use futures_util::StreamExt;

    #[futures_test::test]
    async fn stream_writer_matches_buffered_writer() {
        let mut writer = MultipartWriter::new_with_boundary("test-boundary");
        writer
            .add("first part".as_bytes(), "Content-Type: text/plain")
            .unwrap();
        writer
            .add("second part".as_bytes(), "Content-Type: text/plain")
            .unwrap();
        writer.finish().unwrap();

        let parts = stream::iter(vec![
            Ok::<_, std::io::Error>((
                "Content-Type: text/plain".to_string(),
                stream::iter(vec![
                    Ok(Bytes::from_static(b"first ")),
                    Ok(Bytes::from_static(b"part")),
                ]),
            )),
            Ok((
                "Content-Type: text/plain".to_string(),
                stream::iter(vec![Ok(Bytes::from_static(b"second part"))]),
            )),
        ]);
        let streamed: Vec<u8> = MultipartStreamWriter::new_with_boundary("test-boundary")
            .stream(parts)
            .map(|chunk| chunk.unwrap())
            .collect::<Vec<_>>()
            .await
            .concat();

        assert_eq!(streamed, writer.data);
    }

    #[futures_test::test]
    async fn stream_writer_output_is_readable() {
        let parts = stream::iter(vec![Ok::<_, std::io::Error>((
            "Content-Type: text/plain".to_string(),
            stream::iter(vec![Ok(Bytes::from_static(b"hello world"))]),
        ))]);
        let chunks: Vec<_> = MultipartStreamWriter::new_with_boundary("test-boundary")
            .stream(parts)
            .map(|chunk| chunk.map_err(|_| crate::MultipartError::PollingDataFailed))
            .collect()
            .await;

        let reader = MultipartReader::from_stream_with_boundary_and_type(
            stream::iter(chunks),
            "test-boundary",
            crate::MultipartType::Related,
        )
        .unwrap();
        let items: Vec<_> = reader.collect().await;
        assert_eq!(items.len(), 1);
        let item = items[0].as_ref().unwrap();
        assert_eq!(
            item.headers[0],
            ("Content-Type".to_owned(), "text/plain".to_owned())
        );
        assert_eq!(item.data.as_ref(), b"hello world");
    }
}
