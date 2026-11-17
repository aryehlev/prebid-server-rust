//! Go: `net/http/status.go`.

macro_rules! statuses {
    ($($(#[$doc:meta])* $name:ident = $code:literal, $text:literal;)*) => {
        $(
            $(#[$doc])*
            #[doc = concat!(
                "Go: `http.", stringify!($name), "` (", stringify!($code), " ", $text, ")."
            )]
            pub const $name: u16 = $code;
        )*

        /// Go: `http.StatusText`: the text for the HTTP status code, or `""` when unknown.
        #[must_use]
        pub const fn status_text(code: u16) -> &'static str {
            match code {
                $($code => $text,)*
                _ => "",
            }
        }
    };
}

statuses! {
    STATUS_CONTINUE = 100, "Continue";
    STATUS_SWITCHING_PROTOCOLS = 101, "Switching Protocols";
    STATUS_PROCESSING = 102, "Processing";
    STATUS_EARLY_HINTS = 103, "Early Hints";
    STATUS_OK = 200, "OK";
    STATUS_CREATED = 201, "Created";
    STATUS_ACCEPTED = 202, "Accepted";
    STATUS_NON_AUTHORITATIVE_INFO = 203, "Non-Authoritative Information";
    STATUS_NO_CONTENT = 204, "No Content";
    STATUS_RESET_CONTENT = 205, "Reset Content";
    STATUS_PARTIAL_CONTENT = 206, "Partial Content";
    STATUS_MULTI_STATUS = 207, "Multi-Status";
    STATUS_ALREADY_REPORTED = 208, "Already Reported";
    STATUS_IM_USED = 226, "IM Used";
    STATUS_MULTIPLE_CHOICES = 300, "Multiple Choices";
    STATUS_MOVED_PERMANENTLY = 301, "Moved Permanently";
    STATUS_FOUND = 302, "Found";
    STATUS_SEE_OTHER = 303, "See Other";
    STATUS_NOT_MODIFIED = 304, "Not Modified";
    STATUS_USE_PROXY = 305, "Use Proxy";
    STATUS_TEMPORARY_REDIRECT = 307, "Temporary Redirect";
    STATUS_PERMANENT_REDIRECT = 308, "Permanent Redirect";
    STATUS_BAD_REQUEST = 400, "Bad Request";
    STATUS_UNAUTHORIZED = 401, "Unauthorized";
    STATUS_PAYMENT_REQUIRED = 402, "Payment Required";
    STATUS_FORBIDDEN = 403, "Forbidden";
    STATUS_NOT_FOUND = 404, "Not Found";
    STATUS_METHOD_NOT_ALLOWED = 405, "Method Not Allowed";
    STATUS_NOT_ACCEPTABLE = 406, "Not Acceptable";
    STATUS_PROXY_AUTH_REQUIRED = 407, "Proxy Authentication Required";
    STATUS_REQUEST_TIMEOUT = 408, "Request Timeout";
    STATUS_CONFLICT = 409, "Conflict";
    STATUS_GONE = 410, "Gone";
    STATUS_LENGTH_REQUIRED = 411, "Length Required";
    STATUS_PRECONDITION_FAILED = 412, "Precondition Failed";
    STATUS_REQUEST_ENTITY_TOO_LARGE = 413, "Request Entity Too Large";
    STATUS_REQUEST_URI_TOO_LONG = 414, "Request URI Too Long";
    STATUS_UNSUPPORTED_MEDIA_TYPE = 415, "Unsupported Media Type";
    STATUS_REQUESTED_RANGE_NOT_SATISFIABLE = 416, "Requested Range Not Satisfiable";
    STATUS_EXPECTATION_FAILED = 417, "Expectation Failed";
    STATUS_TEAPOT = 418, "I'm a teapot";
    STATUS_MISDIRECTED_REQUEST = 421, "Misdirected Request";
    STATUS_UNPROCESSABLE_ENTITY = 422, "Unprocessable Entity";
    STATUS_LOCKED = 423, "Locked";
    STATUS_FAILED_DEPENDENCY = 424, "Failed Dependency";
    STATUS_TOO_EARLY = 425, "Too Early";
    STATUS_UPGRADE_REQUIRED = 426, "Upgrade Required";
    STATUS_PRECONDITION_REQUIRED = 428, "Precondition Required";
    STATUS_TOO_MANY_REQUESTS = 429, "Too Many Requests";
    STATUS_REQUEST_HEADER_FIELDS_TOO_LARGE = 431, "Request Header Fields Too Large";
    STATUS_UNAVAILABLE_FOR_LEGAL_REASONS = 451, "Unavailable For Legal Reasons";
    STATUS_INTERNAL_SERVER_ERROR = 500, "Internal Server Error";
    STATUS_NOT_IMPLEMENTED = 501, "Not Implemented";
    STATUS_BAD_GATEWAY = 502, "Bad Gateway";
    STATUS_SERVICE_UNAVAILABLE = 503, "Service Unavailable";
    STATUS_GATEWAY_TIMEOUT = 504, "Gateway Timeout";
    STATUS_HTTP_VERSION_NOT_SUPPORTED = 505, "HTTP Version Not Supported";
    STATUS_VARIANT_ALSO_NEGOTIATES = 506, "Variant Also Negotiates";
    STATUS_INSUFFICIENT_STORAGE = 507, "Insufficient Storage";
    STATUS_LOOP_DETECTED = 508, "Loop Detected";
    STATUS_NOT_EXTENDED = 510, "Not Extended";
    STATUS_NETWORK_AUTHENTICATION_REQUIRED = 511, "Network Authentication Required";
}

/// Go: `bodyAllowedForStatus` (RFC 7230 section 3.3).
pub(super) const fn body_allowed_for_status(status: u16) -> bool {
    !matches!(status, 100..=199 | 204 | 304)
}
