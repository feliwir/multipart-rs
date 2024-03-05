#[derive(Debug)]
pub enum MultipartError {
    // Missing Content-Type header
    NoContentType,

    // Invalid boundary
    InvalidBoundary,

    // Invalid Content-Type
    InvalidContentType,

    // Invalid Multipart type
    InvalidMultipartType,

    // Invalid Item header
    InvalidItemHeader,

    // Failed to poll data from the stream
    PollingDataFailed,
}
