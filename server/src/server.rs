use std::marker::PhantomData;

use http_body_util::BodyExt;
use hyper::{Request, Response, service::Service};
use tokio::io::AsyncWriteExt;

use crate::constants;

#[derive(Clone)]
pub struct SliceBreadServer<B> {
    _phantom: PhantomData<B>,
    base_files_dir: String,
}

impl<B> SliceBreadServer<B> {
    pub fn new(dir: String) -> Self {
        Self {
            _phantom: PhantomData,
            base_files_dir: dir,
        }
    }
}

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
        tracing::error!(%value, "Internal server error during upload");
        SliceBreadServerError::IoError(value)
    }
}

impl From<hyper::http::Error> for SliceBreadServerError {
    fn from(value: hyper::http::Error) -> Self {
        tracing::error!(%value, "Internal server error during upload");
        SliceBreadServerError::HyperError(value)
    }
}

fn get_header<T: std::str::FromStr>(
    headers: &hyper::HeaderMap,
    key: &str,
) -> Result<T, SliceBreadServerError> {
    headers
        .get(key)
        .ok_or_else(|| SliceBreadServerError::BadRequest(format!("Missing header: {}", key)))?
        .to_str()
        .map_err(|_| SliceBreadServerError::BadRequest(format!("Invalid header format: {}", key)))?
        .parse::<T>()
        .map_err(|_| SliceBreadServerError::BadRequest(format!("Invalid header value: {}", key)))
}

impl<B> Service<Request<B>> for SliceBreadServer<B>
where
    B: hyper::body::Body + Send + 'static,
    B::Data: Send,
    B::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
{
    type Response = Response<String>;
    type Error = SliceBreadServerError;
    type Future = std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Self::Response, Self::Error>> + Send>,
    >;

    fn call(&self, req: Request<B>) -> Self::Future {
        let headers = req.headers().clone();
        let base_files_dir = self.base_files_dir.clone();

        Box::pin(async move {
            let body = req
                .collect()
                .await
                .map_err(|e| {
                    SliceBreadServerError::InternalServerError(format!(
                        "Failed to read body: {}",
                        e.into()
                    ))
                })?
                .to_bytes();
            let file_id: String = get_header(&headers, constants::HEADER_FILE_ID)?;
            let chunk_index: usize = get_header(&headers, constants::HEADER_CHUNK_INDEX)?;
            let total_chunks: usize = get_header(&headers, constants::HEADER_TOTAL_CHUNKS)?;
            let file_name: String = get_header(&headers, constants::HEADER_FILE_NAME)?;

            tracing::info!(file_id = %file_id, "Received chunk");
            tracing::debug!("Received chunk index: {}", chunk_index);

            if chunk_index > total_chunks {
                tracing::warn!(chunk_index, total_chunks, "Invalid chunk index");
                return Err(SliceBreadServerError::BadRequest(format!(
                    "Invalid {}: {} > {}: {}",
                    constants::HEADER_CHUNK_INDEX,
                    chunk_index,
                    constants::HEADER_TOTAL_CHUNKS,
                    total_chunks
                )));
            }

            if total_chunks == 0 {
                return Err(SliceBreadServerError::BadRequest(
                    "Total chunks must be at least 1".to_string(),
                ));
            }

            let upload_dir = format!("{}/{}/", base_files_dir, file_id);
            tracing::debug!(upload_dir = %upload_dir, "Creating upload directory");
            tokio::fs::create_dir_all(upload_dir).await?;

            let chunk_file = format!("{}/{}/chunk_{}.bin", base_files_dir, file_id, chunk_index);
            let mut file = tokio::fs::File::create(chunk_file).await?;
            file.write_all(&body).await?;
            file.flush().await?;

            let is_last_chunk = chunk_index == total_chunks - 1;

            if is_last_chunk {
                for i in 0..total_chunks {
                    let chunk_file = format!("{}/{}/chunk_{}.bin", base_files_dir, file_id, i);

                    if !tokio::fs::try_exists(&chunk_file).await? {
                        tracing::warn!(%file_id, missing_chunk = i, "Missing chunk during finalization");
                        return Err(SliceBreadServerError::BadRequest(format!(
                            "Missing chunk: {}",
                            i
                        )));
                    }
                }

                tracing::info!(%file_id, "All chunks received, assembling final file");
                let mut file = tokio::fs::File::create(format!(
                    "{}/{}/{}",
                    base_files_dir, file_id, file_name
                ))
                .await?;
                println!("fileoutput: {:?}", file);
                for i in 0..total_chunks {
                    let chunk_file = format!("{}/{}/chunk_{}.bin", base_files_dir, file_id, i);
                    let chunk_bytes = tokio::fs::read(&chunk_file).await?;
                    file.write_all(&chunk_bytes).await?;
                    tokio::fs::remove_file(chunk_file).await?;
                }
                file.flush().await?;

                tracing::info!(%file_id, file_name = %file_name, "Upload complete and file assembled");
            }

            Ok(Response::builder()
                .status(201)
                .body("File uploaded successfuly".to_string())?)
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use bytes::Bytes;
    use http_body_util::Full;
    use hyper::{Request, service::Service};
    use tempdir::TempDir;
    use tokio::fs;

    use crate::server::{SliceBreadServer, SliceBreadServerError};

    #[tokio::test]
    async fn test_full_upload_success() {
        let temp_dir = TempDir::new("upload_test").expect("create temp dir failed");
        let upload_dir = temp_dir.path().join("uploads");
        tokio::fs::create_dir_all(&upload_dir).await.unwrap();

        let service =
            SliceBreadServer::<Full<Bytes>>::new(upload_dir.to_str().unwrap().to_string());

        let file_id = "test123";
        let file_name = "hello.txt";

        // Upload chunk 0
        let req0 = Request::builder()
            .method("POST")
            .header("X-File-Id", file_id)
            .header("X-File-Name", file_name)
            .header("X-Chunk-Index", "0")
            .header("X-Total-Chunks", "2")
            .body(Full::new(Bytes::from("Hello, ".to_string())).into())
            .unwrap();

        let res = service.call(req0).await.unwrap();
        assert_eq!(res.status(), 201);

        let chunk_path = upload_dir.join(file_id).join("chunk_0.bin");
        let written = tokio::fs::read_to_string(chunk_path).await.unwrap();
        assert_eq!(written, "Hello, ");

        // Upload chunk 1 (final chunk)
        let req1 = Request::builder()
            .method("POST")
            .header("X-File-Id", file_id)
            .header("X-File-Name", file_name)
            .header("X-Chunk-Index", "1")
            .header("X-Total-Chunks", "2")
            .body(Full::new(Bytes::from("World!".to_string())).into())
            .unwrap();

        let res = service.call(req1).await.unwrap();
        assert_eq!(res.status(), 201);

        // Check final file content
        let final_path = upload_dir.join(file_id).join(file_name);
        let content = fs::read_to_string(final_path).await.unwrap();
        assert_eq!(content, "Hello, World!");
    }

    #[tokio::test]
    async fn test_single_chunk_file_upload() {
        let temp_dir = TempDir::new("upload_test").expect("create temp dir failed");
        let upload_dir = temp_dir.path().join("uploads");
        tokio::fs::create_dir_all(&upload_dir).await.unwrap();

        let service =
            SliceBreadServer::<Full<Bytes>>::new(upload_dir.to_str().unwrap().to_string());

        let file_id = "test1238";
        let file_name = "hello.txt";

        // Upload chunk 0
        let req0 = Request::builder()
            .method("POST")
            .header("X-File-Id", file_id)
            .header("X-File-Name", file_name)
            .header("X-Chunk-Index", "0")
            .header("X-Total-Chunks", "1")
            .body(Full::new(Bytes::from("Hello, World!".to_string())).into())
            .unwrap();

        let res = service.call(req0).await.unwrap();
        assert_eq!(res.status(), 201);

        // Check final file content
        let final_path = upload_dir.join(file_id).join(file_name);
        let content = fs::read_to_string(final_path).await.unwrap();
        assert_eq!(content, "Hello, World!");
    }

    #[tokio::test]
    async fn test_missing_x_file_id_header() {
        let service = SliceBreadServer::<Full<Bytes>>::new(String::from("uploads"));

        let file_id = "test12321";
        let file_name = "hello.txt";

        // Upload chunk 0
        let req0 = Request::builder()
            .method("POST")
            .header("X-File-Name", file_name)
            .header("X-Chunk-Index", "0")
            .header("X-Total-Chunks", "1    ")
            .body(Full::new(Bytes::from("Hello, World!".to_string())).into())
            .unwrap();

        let res = service.call(req0).await;
        assert!(
            matches!(res.unwrap_err(), SliceBreadServerError::BadRequest(ref msg) if msg.contains("Missing header"))
        );

        // Check final file content
        let final_path = format!("uploads/{}/{}", file_id, file_name);
        let exists = fs::try_exists(final_path).await.unwrap();
        assert!(!exists);
    }

    #[tokio::test]
    async fn test_missing_x_file_name_header() {
        let service = SliceBreadServer::<Full<Bytes>>::new(String::from("uploads"));

        let file_id = "test12321";
        let file_name = "hello.txt";

        // Upload chunk 0
        let req0 = Request::builder()
            .header("X-File-Id", file_id)
            .header("X-Chunk-Index", "0")
            .header("X-Total-Chunks", "1")
            .body(Full::new(Bytes::from("Hello, World!".to_string())).into())
            .unwrap();

        let res = service.call(req0).await;
        assert!(
            matches!(res.unwrap_err(), SliceBreadServerError::BadRequest(ref msg) if msg.contains("Missing header"))
        );

        // Check final file content
        let final_path = format!("uploads/{}/{}", file_id, file_name);
        let exists = fs::try_exists(final_path).await.unwrap();
        assert!(!exists);
    }

    #[tokio::test]
    async fn test_missing_x_chunk_index_header() {
        let service = SliceBreadServer::<Full<Bytes>>::new(String::from("uploads"));

        let file_id = "test12321";
        let file_name = "hello.txt";

        // Upload chunk 0
        let req0 = Request::builder()
            .header("X-File-Id", file_id)
            .header("X-File-Name", file_name)
            .header("X-Total-Chunks", "2")
            .body(Full::new(Bytes::from("Hello, World!".to_string())).into())
            .unwrap();

        let res = service.call(req0).await;
        assert!(
            matches!(res.unwrap_err(), SliceBreadServerError::BadRequest(ref msg) if msg.contains("Missing header"))
        );

        // Check final file content
        let final_path = format!("uploads/{}/{}", file_id, file_name);
        let exists = fs::try_exists(final_path).await.unwrap();
        assert!(!exists);
    }

    #[tokio::test]
    async fn test_missing_x_total_chunks_header() {
        let service = SliceBreadServer::<Full<Bytes>>::new(String::from("uploads"));

        let file_id = "test12321";
        let file_name = "hello.txt";

        // Upload chunk 0
        let req0 = Request::builder()
            .header("X-File-Id", file_id)
            .header("X-File-Name", file_name)
            .header("X-Chunk-Index", "0")
            .body(Full::new(Bytes::from("Hello, World!".to_string())).into())
            .unwrap();

        let res = service.call(req0).await;
        assert!(
            matches!(res.unwrap_err(), SliceBreadServerError::BadRequest(ref msg) if msg.contains("Missing header"))
        );

        // Check final file content
        let final_path = format!("uploads/{}/{}", file_id, file_name);
        let exists = fs::try_exists(final_path).await.unwrap();
        assert!(!exists);
    }

    #[tokio::test]
    async fn test_invalid_chunk_index_format() {
        let service = SliceBreadServer::<Full<Bytes>>::new(String::from("uploads"));

        let file_id = "test12321";
        let file_name = "hello.txt";

        // Upload chunk 0
        let req0 = Request::builder()
            .header("X-File-Id", file_id)
            .header("X-File-Name", file_name)
            .header("X-Chunk-Index", "one")
            .header("X-Total-Chunks", "2")
            .body(Full::new(Bytes::from("Hello, World!".to_string())).into())
            .unwrap();

        let res = service.call(req0).await;
        assert!(
            matches!(res.unwrap_err(), SliceBreadServerError::BadRequest(ref msg) if msg.contains("Invalid header"))
        );

        // Check final file content
        let final_path = format!("uploads/{}/{}", file_id, file_name);
        let exists = fs::try_exists(final_path).await.unwrap();
        assert!(!exists);
    }

    #[tokio::test]
    async fn test_invalid_total_chunks_format() {
        let service = SliceBreadServer::<Full<Bytes>>::new(String::from("uploads"));

        let file_id = "test12321";
        let file_name = "hello.txt";

        // Upload chunk 0
        let req0 = Request::builder()
            .header("X-File-Id", file_id)
            .header("X-File-Name", file_name)
            .header("X-Chunk-Index", "0")
            .header("X-Total-Chunks", "one")
            .body(Full::new(Bytes::from("Hello, World!".to_string())).into())
            .unwrap();

        let res = service.call(req0).await;
        assert!(
            matches!(res.unwrap_err(), SliceBreadServerError::BadRequest(ref msg) if msg.contains("Invalid header"))
        );

        // Check final file content
        let final_path = format!("uploads/{}/{}", file_id, file_name);
        let exists = fs::try_exists(final_path).await.unwrap();
        assert!(!exists);
    }

    #[tokio::test]
    async fn test_chunk_index_greater_than_total_chunks() {
        let service = SliceBreadServer::<Full<Bytes>>::new(String::from("uploads"));

        let file_id = "test123";
        let file_name = "hello.txt";

        // Upload chunk 0
        let req0 = Request::builder()
            .method("POST")
            .header("X-File-Id", file_id)
            .header("X-File-Name", file_name)
            .header("X-Chunk-Index", "2")
            .header("X-Total-Chunks", "1")
            .body(Full::new(Bytes::from("Hello, ".to_string())).into())
            .unwrap();

        let res = service.call(req0).await;
        assert!(
            matches!(res.unwrap_err(), SliceBreadServerError::BadRequest(ref msg) if msg.eq("Invalid X-Chunk-Index: 2 > X-Total-Chunks: 1"))
        );
    }

    #[tokio::test]
    async fn test_missing_intermediate_chunk_on_finalization() {
        let temp_dir = TempDir::new("upload_test").expect("create temp dir failed");
        let upload_dir = temp_dir.path().join("uploads");
        tokio::fs::create_dir_all(&upload_dir).await.unwrap();

        let service =
            SliceBreadServer::<Full<Bytes>>::new(upload_dir.to_str().unwrap().to_string());

        let file_id = "test12663";
        let file_name = "hello.txt";

        // Upload chunk 0
        let req0 = Request::builder()
            .method("POST")
            .header("X-File-Id", file_id)
            .header("X-File-Name", file_name)
            .header("X-Chunk-Index", "0")
            .header("X-Total-Chunks", "3")
            .body(Full::new(Bytes::from("Hello, ".to_string())).into())
            .unwrap();

        let res = service.call(req0).await.unwrap();
        assert_eq!(res.status(), 201);

        // Upload final chunk
        let req1 = Request::builder()
            .method("POST")
            .header("X-File-Id", file_id)
            .header("X-File-Name", file_name)
            .header("X-Chunk-Index", "2")
            .header("X-Total-Chunks", "3")
            .body(Full::new(Bytes::from("World!".to_string())).into())
            .unwrap();

        let res = service.call(req1).await;
        assert!(
            matches!(res.unwrap_err(), SliceBreadServerError::BadRequest(ref msg) if msg.eq("Missing chunk: 1"))
        );
    }

    #[tokio::test]
    async fn test_write_failure_returns_internal_server_error() {
        let temp_dir = TempDir::new("upload_test").expect("create temp dir failed");
        let upload_dir = temp_dir.path().join("uploads");
        tokio::fs::create_dir_all(&upload_dir).await.unwrap();

        let service =
            SliceBreadServer::<Full<Bytes>>::new(upload_dir.to_str().unwrap().to_string());

        let file_id = "test123";
        let file_name = "hello.txt";

        let conflict_path = upload_dir.join(file_id).join(file_name);
        tokio::fs::create_dir_all(&conflict_path).await.unwrap();

        // Upload chunk 0
        let req0 = Request::builder()
            .method("POST")
            .header("X-File-Id", file_id)
            .header("X-File-Name", file_name)
            .header("X-Chunk-Index", "0")
            .header("X-Total-Chunks", "1")
            .body(Full::new(Bytes::from("Hello, ".to_string())).into())
            .unwrap();

        let res = service.call(req0).await;
        assert!(
            matches!(res.unwrap_err(), SliceBreadServerError::IoError(ref e) if e.to_string().contains("Is a directory"))
        );
    }

    #[tokio::test]
    async fn test_output_file_creation_failure() {
        let temp_dir = TempDir::new("upload_test").expect("create temp dir failed");
        let upload_dir = temp_dir.path().join("uploads");
        tokio::fs::create_dir_all(&upload_dir).await.unwrap();

        let service =
            SliceBreadServer::<Full<Bytes>>::new(upload_dir.to_str().unwrap().to_string());

        let file_id = "test445562";
        let file_name = "hello.txt";
        let file_id_path = upload_dir.join(file_id);
        tokio::fs::create_dir_all(&file_id_path).await.unwrap();

        let chunk_file = file_id_path.join(file_name);
        tokio::fs::create_dir_all(&chunk_file).await.unwrap(); // <— key change

        // Upload chunk 0
        let req = Request::builder()
            .method("POST")
            .header("X-File-Id", file_id)
            .header("X-File-Name", file_name)
            .header("X-Chunk-Index", "0")
            .header("X-Total-Chunks", "1")
            .body(Full::new(Bytes::from("Hello, ".to_string())).into())
            .unwrap();

        let res = service.call(req).await;

        assert!(
            matches!(res.unwrap_err(), SliceBreadServerError::IoError(ref e) if e.to_string().contains("Is a directory"))
        );
    }

    #[tokio::test]
    async fn test_chunks_are_deleted_after_merge() {
        let temp_dir = TempDir::new("upload_test").unwrap();
        let upload_dir = temp_dir.path().join("uploads");
        tokio::fs::create_dir_all(&upload_dir).await.unwrap();

        let service =
            SliceBreadServer::<Full<Bytes>>::new(upload_dir.to_str().unwrap().to_string());

        let file_id = "file123";
        let file_name = "final.txt";

        // Send single chunk (merge will happen)
        let req = Request::builder()
            .method("POST")
            .header("X-File-Id", file_id)
            .header("X-File-Name", file_name)
            .header("X-Chunk-Index", "0")
            .header("X-Total-Chunks", "1")
            .body(Full::new(Bytes::from("Hello, world!")).into())
            .unwrap();

        let res = service.call(req).await.unwrap();
        assert_eq!(res.status(), 201);

        let chunk_path = upload_dir.join(file_id).join("chunk_0.bin");
        assert!(!chunk_path.exists());
    }

    #[tokio::test]
    async fn test_final_file_has_correct_content() {}

    #[tokio::test]
    async fn test_empty_chunk_upload() {
        let temp_dir = TempDir::new("upload_test").unwrap();
        let upload_dir = temp_dir.path().join("uploads");
        tokio::fs::create_dir_all(&upload_dir).await.unwrap();

        let service =
            SliceBreadServer::<Full<Bytes>>::new(upload_dir.to_str().unwrap().to_string());

        let file_id = "fileEmpty";
        let file_name = "empty.txt";

        let req = Request::builder()
            .method("POST")
            .header("X-File-Id", file_id)
            .header("X-File-Name", file_name)
            .header("X-Chunk-Index", "0")
            .header("X-Total-Chunks", "1")
            .body(Full::new(Bytes::new()).into())
            .unwrap();

        let res = service.call(req).await.unwrap();
        assert_eq!(res.status(), 201);

        let final_path = upload_dir.join(file_id).join(file_name);
        let written = tokio::fs::read(final_path).await.unwrap();
        assert_eq!(written.len(), 0);
    }

    #[tokio::test]
    async fn test_duplicate_chunk_upload() {
        let temp_dir = TempDir::new("upload_test").unwrap();
        let upload_dir = temp_dir.path().join("uploads");
        tokio::fs::create_dir_all(&upload_dir).await.unwrap();

        let service =
            SliceBreadServer::<Full<Bytes>>::new(upload_dir.to_str().unwrap().to_string());

        let file_id = "fileDup";
        let file_name = "dup.txt";

        let req = |data: &str| {
            Request::builder()
                .method("POST")
                .header("X-File-Id", file_id)
                .header("X-File-Name", file_name)
                .header("X-Chunk-Index", "0")
                .header("X-Total-Chunks", "1")
                .body(Full::new(Bytes::from(data.to_string())).into())
                .unwrap()
        };

        // First upload
        let res1 = service.call(req("first")).await.unwrap();
        assert_eq!(res1.status(), 201);

        // Second upload with same chunk index
        let res2 = service.call(req("second")).await.unwrap();
        assert_eq!(res2.status(), 201);

        let final_path = upload_dir.join(file_id).join(file_name);
        let content = tokio::fs::read_to_string(final_path).await.unwrap();

        // Depending on implementation: expect "second" or "first"
        assert!(["first", "second"].contains(&content.as_str()));
    }

    #[tokio::test]
    async fn test_large_chunk_upload() {
        let temp_dir = TempDir::new("upload_test").unwrap();
        let upload_dir = temp_dir.path().join("uploads");
        tokio::fs::create_dir_all(&upload_dir).await.unwrap();

        let service =
            SliceBreadServer::<Full<Bytes>>::new(upload_dir.to_str().unwrap().to_string());

        let file_id = "fileLarge";
        let file_name = "large.txt";

        let big_data = "A".repeat(10 * 1024 * 1024); // 10 MB

        let req = Request::builder()
            .method("POST")
            .header("X-File-Id", file_id)
            .header("X-File-Name", file_name)
            .header("X-Chunk-Index", "0")
            .header("X-Total-Chunks", "1")
            .body(Full::new(Bytes::from(big_data.clone())).into())
            .unwrap();

        let res = service.call(req).await.unwrap();
        assert_eq!(res.status(), 201);

        let final_path = upload_dir.join(file_id).join(file_name);
        let written = tokio::fs::read_to_string(final_path).await.unwrap();
        assert_eq!(written.len(), big_data.len());
    }

    #[tokio::test]
    async fn test_concurrent_uploads_same_file_id() {
        use futures_util::future::join_all;

        let temp_dir = TempDir::new("upload_test").unwrap();
        let upload_dir = temp_dir.path().join("uploads");
        tokio::fs::create_dir_all(&upload_dir).await.unwrap();

        let service = Arc::new(SliceBreadServer::<Full<Bytes>>::new(
            upload_dir.to_str().unwrap().to_string(),
        ));

        let file_id = "fileConcurrent";
        let file_name = "multi.txt";

        let chunks = vec!["One", "Two", "Three", "Four"];

        let futures = chunks.iter().enumerate().map(|(i, chunk)| {
            let service = Arc::clone(&service);
            let req = Request::builder()
                .method("POST")
                .header("X-File-Id", file_id)
                .header("X-File-Name", file_name)
                .header("X-Chunk-Index", i.to_string())
                .header("X-Total-Chunks", chunks.len().to_string())
                .body(Full::new(Bytes::from(chunk.to_string())).into())
                .unwrap();
            async move {
                let res = service.call(req).await;
                assert!(res.is_ok());
            }
        });

        join_all(futures).await;

        let final_path = upload_dir.join(file_id).join(file_name);
        let result = tokio::fs::read_to_string(final_path).await.unwrap();

        for chunk in &chunks {
            assert!(result.contains(chunk));
        }
    }
}
