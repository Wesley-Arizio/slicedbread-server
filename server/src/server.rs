use http_body_util::BodyExt;
use hyper::{
    Request, Response,
    body::{Body, Bytes, Incoming},
    service::Service,
};
use tokio::io::AsyncWriteExt;

#[derive(Clone)]
pub struct SliceBreadServer {}

#[derive(Debug)]
pub enum SliceBreadServerError {
    InternalServerError(String),
    BadRequest(String),
    IoError(std::io::Error),
    HyperError(hyper::http::Error),
}

impl std::error::Error for SliceBreadServerError {}

impl std::fmt::Display for SliceBreadServerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InternalServerError(msg) => write!(f, "Internal Server Error: {}", msg),
            Self::BadRequest(msg) => write!(f, "Bad Request: {}", msg),
            Self::IoError(err) => write!(f, "IO Error: {}", err),
            Self::HyperError(err) => write!(f, "Hyper Error: {}", err),
        }
    }
}

impl From<std::io::Error> for SliceBreadServerError {
    fn from(value: std::io::Error) -> Self {
        SliceBreadServerError::IoError(value)
    }
}

impl From<hyper::http::Error> for SliceBreadServerError {
    fn from(value: hyper::http::Error) -> Self {
        SliceBreadServerError::HyperError(value)
    }
}

impl Service<Request<Incoming>> for SliceBreadServer {
    type Response = Response<String>;
    type Error = SliceBreadServerError;
    type Future = std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Self::Response, Self::Error>> + Send>,
    >;

    fn call(&self, req: Request<Incoming>) -> Self::Future {
        let headers = req.headers().clone();

        Box::pin(async move {
            let body = req.collect().await.unwrap().to_bytes();
            let maybe_file_id = headers
                .get("X-File-Id")
                .and_then(|v| v.to_str().ok())
                .map(String::from);

            let Some(file_id) = maybe_file_id else {
                return Ok(Response::builder()
                    .status(400)
                    .body("X-File-Id header is missing".to_string())?);
            };

            let maybe_chunk_index = headers
                .get("X-Chunk-Index")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<usize>().ok());

            let Some(chunk_index) = maybe_chunk_index else {
                return Ok(Response::builder()
                    .status(400)
                    .body("X-Chunk-Index header is missing".to_string())?);
            };

            let maybe_total_chunks = headers
                .get("X-Total-Chunks")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<usize>().ok());

            let Some(total_chunks) = maybe_total_chunks else {
                return Ok(Response::builder()
                    .status(400)
                    .body("X-Total-Chunks header is missing".to_string())?);
            };

            let maybe_file_name = headers
                .get("X-File-Name")
                .and_then(|v| v.to_str().ok())
                .map(String::from);

            let Some(file_name) = maybe_file_name else {
                return Ok(Response::builder()
                    .status(400)
                    .body("X-File-Name header is missing".to_string())?);
            };

            if chunk_index > total_chunks {
                return Ok(Response::builder().status(400).body(format!(
                    "Invalid chunk_index: {} >= total_chunks: {}",
                    chunk_index, total_chunks
                ))?);
            }

            tokio::fs::create_dir_all(format!("uploads/{}/", file_id)).await?;

            let chunk_file = format!("uploads/{}/chunk_{}.bin", file_id, chunk_index);
            let mut file = tokio::fs::File::create(chunk_file).await?;
            file.write_all(&body).await?;
            file.flush().await?;

            let is_last_chunk = chunk_index == total_chunks;

            if is_last_chunk {
                for i in 0..=total_chunks {
                    let chunk_file = format!("uploads/{}/chunk_{}.bin", file_id, i);
                    if !tokio::fs::try_exists(&chunk_file).await? {
                        return Ok(Response::builder()
                            .status(400)
                            .body(format!("Missing chunk {}", i))?);
                    }
                }

                let mut file =
                    tokio::fs::File::create(format!("uploads/{}/{}", file_id, file_name,)).await?;
                for i in 0..=total_chunks {
                    let chunk_file = format!("uploads/{}/chunk_{}.bin", file_id, i);
                    let chunk_bytes = tokio::fs::read(&chunk_file).await?;
                    file.write_all(&chunk_bytes).await?;
                    tokio::fs::remove_file(chunk_file).await?;
                }
                file.flush().await?;
            }

            Ok(Response::builder()
                .status(201)
                .body("File uploaded successfuly".to_string())?)
        })
    }
}
