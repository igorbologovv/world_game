use crate::error::ApiError;
use axum::{
    body::Bytes,
    http::{HeaderValue, StatusCode, header::CONTENT_TYPE},
    response::{IntoResponse, Response},
};
use prost::Message;

pub const PROTOBUF_CONTENT_TYPE: &str = "application/x-protobuf";

pub fn decode_proto<T>(body: &Bytes) -> Result<T, ApiError>
where
    T: Message + Default,
{
    T::decode(body.clone())
        .map_err(|_| ApiError::bad_request("bad_protobuf", "invalid protobuf body"))
}

pub fn proto_response<T>(status: StatusCode, message: &T) -> Response
where
    T: Message,
{
    let mut body = Vec::new();
    if let Err(error) = message.encode(&mut body) {
        tracing::error!(%error, "failed to encode protobuf response");
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }

    (
        status,
        [(
            CONTENT_TYPE,
            HeaderValue::from_static(PROTOBUF_CONTENT_TYPE),
        )],
        body,
    )
        .into_response()
}
