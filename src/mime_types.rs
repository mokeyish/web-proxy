use mime::*;
use std::str::FromStr;

pub fn get_mime_type(ext: &str) -> Mime {
    Mime::from_str(match ext {
        "html" | "htm" | "shtml" => "text/html",
        "css" => "text/css",
        "xml" => "text/xml",
        "gif" => "image/gif",
        "jpeg" | "jpg" => "image/jpeg",
        "js" => "application/javascript",
        "txt" => "text/plain",

        "png" => "image/png",

        "json" => "application/json",
        _ => "application/octet-stream"
    }).unwrap_or(APPLICATION_OCTET_STREAM)
}



