use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ErrorBody {
    pub error: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusCode {
    Ok = 200,
    Created = 201,
    BadRequest = 400,
    NotFound = 404,
    MethodNotAllowed = 405,
    InternalServerError = 500,
}

impl StatusCode {
    pub fn reason_phrase(self) -> &'static str {
        match self {
            Self::Ok => "OK",
            Self::Created => "Created",
            Self::BadRequest => "Bad Request",
            Self::NotFound => "Not Found",
            Self::MethodNotAllowed => "Method Not Allowed",
            Self::InternalServerError => "Internal Server Error",
        }
    }
}

#[derive(Debug, Clone)]
pub enum WebError {
    NotFound(String),
    BadRequest(String),
    Internal(String),
}

impl WebError {
    pub fn not_found(message: impl Into<String>) -> Self {
        Self::NotFound(message.into())
    }

    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::BadRequest(message.into())
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::Internal(message.into())
    }
}

impl From<crate::Error> for WebError {
    fn from(value: crate::Error) -> Self {
        match value {
            crate::Error::TeamNotFound { name } => {
                Self::NotFound(format!("team '{name}' not found"))
            }
            crate::Error::MemberNotFound { team, member } => {
                Self::NotFound(format!("member '{member}' not found in team '{team}'"))
            }
            crate::Error::InvalidName { name, reason } => {
                Self::BadRequest(format!("invalid name '{name}': {reason}"))
            }
            other => Self::Internal(other.to_string()),
        }
    }
}

impl WebError {
    pub fn status_code(&self) -> StatusCode {
        match self {
            WebError::NotFound(_) => StatusCode::NotFound,
            WebError::BadRequest(_) => StatusCode::BadRequest,
            WebError::Internal(_) => StatusCode::InternalServerError,
        }
    }

    pub fn message(&self) -> &str {
        match self {
            WebError::NotFound(error) | WebError::BadRequest(error) | WebError::Internal(error) => {
                error
            }
        }
    }
}
