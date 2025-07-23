# SliceBread Server

A file upload server built with [Hyper](https://github.com/hyperium/hyper) and `tokio`, designed for reliable chunked uploads. Each uploaded file is processed in parts and merged safely on the server.

---

## ✨ Features

- Chunked file uploads
- Automatic merge after last chunk
- Clean-up of temporary chunk files
- Concurrent upload support
- Custom error handling with meaningful HTTP responses
- Built with async Rust using `tokio` and `hyper`

---

## 🛠️ Usage

Before running the project, navigate to the `server/` folder and set up a `.env` file based on the provided `.env.example`

```bash
cd server
cp .env.example .env
cargo run --release
```

By default, uploads are saved to the `uploads/` directory.

---

## 📦 API

### `POST /`

**Headers:**

- `X-File-Id`: Unique identifier for the upload session
- `X-File-Name`: Name of the file being uploaded
- `X-Chunk-Index`: Current chunk index (0-based)
- `X-Total-Chunks`: Total number of chunks expected

**Body:**

Raw binary data for the current chunk.

**Response:**

- `200 OK`: Chunk accepted
- `400 Bad Request`: If any of the headers are missing or are in invalid format
- `500 Internal Server Error`: If any IO or server error occurs

---

## 🧪 Running Tests

```bash
cargo test
```

Unit tests are defined in `src/server.rs` and cover:

- File creation errors
- Chunk merging correctness
- Cleanup behavior
- Large/empty chunk handling
- Duplicate chunk uploads
- Concurrent uploads (different file IDs)

---

## 🔧 Configuration

You can customize upload directories and other parameters via environment variables or config files (see `.env.example`).

---

## 🚧 TODO

- Add file size validation
- Rate limiting or throttling
- Upload session expiration logic
- Authentication middleware

---
