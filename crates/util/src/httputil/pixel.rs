//! Go: `util/httputil/pixel.go`.

/// Go: `httputil.Pixel`, an image served by the event endpoints.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Pixel {
    /// The image bytes (Go: `Content`).
    pub content: &'static [u8],
    /// The `Content-Type` header value (Go: `ContentType`).
    pub content_type: &'static str,
}

/// Go: `httputil.Pixel1x1PNG`: the 1x1 transparent PNG of `util/httputil/pixel.png`.
pub const PIXEL_1X1_PNG: Pixel = Pixel {
    content: include_bytes!("pixel.png"),
    content_type: "image/png",
};
