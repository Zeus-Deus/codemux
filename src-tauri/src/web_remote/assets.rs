//! Authenticated file-asset route for the web-remote server.
//!
//! Backs the browser shim's `convertFileSrc` replacement: the shim maps a
//! local file path to `/api/assets?path=<absolute>` (see `src/remote/shim.ts`),
//! and this route streams that file back with a guessed MIME type so
//! `<img src>` / editor previews resolve on the web client exactly as the
//! desktop `asset:` protocol resolves them.
//!
//! ## Security model — full-file read is by design
//!
//! This route serves ANY file the desktop user can read. That is intentional:
//! a paired web session has desktop-level control — it *is* the desktop. The
//! security boundary is pairing + revocation + the network layer, not per-path
//! ACLs. The desktop webview's own `asset:`/`convertFileSrc` protocol reads
//! arbitrary local files the same way; this is the web-transport equivalent.
//! Access is gated on an approved, non-revoked session (bearer or HttpOnly
//! cookie) plus a same-origin check via [`super::server::require_session`]. Directories and
//! non-regular files return 404 — the route never produces a directory
//! listing.

use std::{io, path::Path};

use axum::http::HeaderMap;
use axum::{
    extract::{Query, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
};
use futures_util::TryStreamExt as _;
use serde::Deserialize;
use tauri::{AppHandle, Runtime};

use super::server::{require_session, session_active};

const ASSET_CHUNK_SIZE: usize = 64 * 1024;

#[derive(Deserialize)]
pub struct AssetQuery {
    path: String,
}

/// `GET /api/assets?path=<absolute>` — auth-gated file streamer.
pub async fn serve<R: Runtime>(
    State(app): State<AppHandle<R>>,
    headers: HeaderMap,
    Query(query): Query<AssetQuery>,
) -> Response {
    let session = match require_session(&app, &headers) {
        Ok(session) => session,
        Err(response) => return response,
    };

    let opened = open_asset(Path::new(&query.path)).await;
    if !session_active(&app, &session.id) {
        return StatusCode::UNAUTHORIZED.into_response();
    }

    match opened {
        Ok(asset) => {
            // Check after each read. A body error aborts the transfer even
            // when compression has removed the Content-Length header.
            let stream = tokio_util::io::ReaderStream::with_capacity(asset.file, ASSET_CHUNK_SIZE)
                .and_then(move |chunk| {
                    std::future::ready(if session_active(&app, &session.id) {
                        Ok(chunk)
                    } else {
                        Err(io::Error::new(
                            io::ErrorKind::PermissionDenied,
                            "asset session revoked",
                        ))
                    })
                });
            let body = axum::body::Body::from_stream(stream);
            asset_response(asset.mime, asset.len, body)
        }
        Err(AssetError::NotFound) => (StatusCode::NOT_FOUND, "not found").into_response(),
    }
}

/// CSP applied when a scriptable asset is opened as a top-level document.
/// `sandbox` gives the document an opaque origin, so script inside a repo's
/// SVG/HTML cannot use the session cookie to reach the authenticated API
/// (e.g. mint a WebSocket ticket). It has no effect on `<img src>` rendering.
const SCRIPTABLE_DOCUMENT_CSP: &str = "default-src 'none'; style-src 'unsafe-inline'; sandbox";

/// Whether a served MIME type can execute script when navigated to directly.
fn is_scriptable_mime(mime: &str) -> bool {
    let essence = mime.split(';').next().unwrap_or("").trim();
    essence == "text/html" || essence.ends_with("+xml") || essence.ends_with("/xml")
}

/// Build the 200 response for an asset. Always disables MIME sniffing so a
/// file served as text or a download is never upgraded to HTML, and
/// sandboxes scriptable document types.
fn asset_response(mime: &'static str, len: u64, body: axum::body::Body) -> Response {
    let mut builder = axum::response::Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, mime)
        .header(header::CONTENT_LENGTH, len)
        .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff");
    if is_scriptable_mime(mime) {
        builder = builder.header(header::CONTENT_SECURITY_POLICY, SCRIPTABLE_DOCUMENT_CSP);
    }
    builder.body(body).unwrap_or_else(|_| {
        (StatusCode::INTERNAL_SERVER_ERROR, "asset build failed").into_response()
    })
}

/// An openable regular file plus the metadata needed to serve it.
struct OpenAsset {
    file: tokio::fs::File,
    mime: &'static str,
    len: u64,
}

enum AssetError {
    /// Missing path, a directory, or any non-regular file.
    NotFound,
}

/// Open a regular file for streaming. Non-files (directories, missing paths,
/// sockets/fifos) map to [`AssetError::NotFound`] so the route never lists a
/// directory or leaks a special-file read.
async fn open_asset(path: &Path) -> Result<OpenAsset, AssetError> {
    let meta = tokio::fs::metadata(path)
        .await
        .map_err(|_| AssetError::NotFound)?;
    if !meta.is_file() {
        return Err(AssetError::NotFound);
    }
    let file = tokio::fs::File::open(path)
        .await
        .map_err(|_| AssetError::NotFound)?;
    Ok(OpenAsset {
        file,
        mime: guess_mime(path),
        len: meta.len(),
    })
}

/// Guess a MIME type from a path's extension. Covers the media/text/font
/// types the app's `convertFileSrc` call sites actually reference (images,
/// video/audio previews, PDFs, markdown/text). Unknown extensions fall back
/// to `application/octet-stream`, which browsers download rather than
/// mis-render. A small hand-rolled table keeps this dependency-free.
fn guess_mime(path: &Path) -> &'static str {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    match ext.as_str() {
        // Images
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "avif" => "image/avif",
        "bmp" => "image/bmp",
        "ico" => "image/x-icon",
        "tif" | "tiff" => "image/tiff",
        "heic" => "image/heic",
        // Video
        "mp4" | "m4v" => "video/mp4",
        "webm" => "video/webm",
        "mov" => "video/quicktime",
        "mkv" => "video/x-matroska",
        "avi" => "video/x-msvideo",
        // Audio
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "ogg" | "oga" => "audio/ogg",
        "m4a" => "audio/mp4",
        "flac" => "audio/flac",
        // Documents / text
        "pdf" => "application/pdf",
        "json" => "application/json",
        "txt" | "log" => "text/plain; charset=utf-8",
        "md" | "markdown" => "text/markdown; charset=utf-8",
        "csv" => "text/csv; charset=utf-8",
        "html" | "htm" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "xml" => "application/xml",
        // Fonts
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn sign_out_aborts_a_gzip_account_asset_stream_and_keeps_code_pairing() {
        use crate::database::DatabaseStore;
        use crate::web_remote::{auth, server};
        use axum::{body::Body, http::Request};
        use futures_util::StreamExt as _;
        use rand::{rngs::StdRng, RngCore, SeedableRng};
        use std::io::Read as _;
        use tauri::Manager as _;
        use tower::ServiceExt as _;

        let app = tauri::test::mock_app();
        let db = DatabaseStore::new_in_memory();
        auth::seed_account_browser(&db, "account", "account-browser");
        db.web_remote_insert_session(
            "paired",
            None,
            None,
            &auth::sha256_hex("paired-browser"),
            true,
        )
        .unwrap();
        app.manage(db);

        let fixture = tempfile::tempdir().unwrap();
        let path = fixture.path().join("asset.txt");
        // Ensure gzip yields data before it can consume the whole input.
        let mut contents = vec![0; ASSET_CHUNK_SIZE * 8];
        StdRng::seed_from_u64(1).fill_bytes(&mut contents);
        tokio::fs::write(&path, &contents).await.unwrap();

        let route = server::router(app.handle().clone());
        let uri = format!("/api/assets?path={}", urlencoding::encode(&path.to_string_lossy()));
        let request = |token| {
            route.clone().oneshot(
                Request::builder()
                    .uri(uri.as_str())
                    .header(header::AUTHORIZATION, token)
                    .header(header::ACCEPT_ENCODING, "gzip")
                    .body(Body::empty())
                    .unwrap(),
            )
        };
        let account = request("Bearer account-browser").await.unwrap();
        let paired = request("Bearer paired-browser").await.unwrap();
        for response in [&account, &paired] {
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(response.headers()[header::CONTENT_ENCODING], "gzip");
            assert!(!response.headers().contains_key(header::CONTENT_LENGTH));
        }
        let mut account = account.into_body().into_data_stream();
        let mut paired = paired.into_body().into_data_stream();
        let account_first = account.next().await.unwrap().unwrap();
        assert!(!account_first.is_empty() && account_first.len() < contents.len());
        let mut paired_bytes = paired.next().await.unwrap().unwrap().to_vec();
        assert!(!paired_bytes.is_empty() && paired_bytes.len() < contents.len());

        app.state::<DatabaseStore>().clear_auth_token().unwrap();

        assert!(account.any(|chunk| std::future::ready(chunk.is_err())).await);
        while let Some(chunk) = paired.next().await {
            paired_bytes.extend(chunk.unwrap());
        }
        let mut decoded = Vec::new();
        flate2::read::GzDecoder::new(paired_bytes.as_slice())
            .read_to_end(&mut decoded)
            .unwrap();
        assert_eq!(decoded, contents);
        assert_eq!(
            request("Bearer account-browser").await.unwrap().status(),
            StatusCode::UNAUTHORIZED
        );
    }

    #[tokio::test]
    async fn serves_regular_file_with_guessed_mime_and_length() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("shot.png");
        tokio::fs::write(&p, b"\x89PNG\r\n\x1a\n").await.unwrap();

        let asset = open_asset(&p).await.map_err(|_| ()).expect("file opens");
        assert_eq!(asset.mime, "image/png");
        assert_eq!(asset.len, 8);
    }

    #[tokio::test]
    async fn directory_is_not_found() {
        let dir = tempfile::tempdir().unwrap();
        assert!(matches!(
            open_asset(dir.path()).await,
            Err(AssetError::NotFound)
        ));
    }

    #[tokio::test]
    async fn missing_path_is_not_found() {
        assert!(matches!(
            open_asset(Path::new("/no/such/file/should/exist/xyz.png")).await,
            Err(AssetError::NotFound)
        ));
    }

    #[test]
    fn every_asset_disables_sniffing_and_scriptable_types_are_sandboxed() {
        for name in ["logo.svg", "page.html", "page.htm", "feed.xml"] {
            let mime = guess_mime(Path::new(name));
            let resp = asset_response(mime, 0, axum::body::Body::empty());
            assert_eq!(resp.headers()[header::X_CONTENT_TYPE_OPTIONS], "nosniff", "{name}");
            assert_eq!(
                resp.headers()[header::CONTENT_SECURITY_POLICY],
                SCRIPTABLE_DOCUMENT_CSP,
                "{name}"
            );
        }
        for name in ["shot.png", "notes.md", "app.js", "doc.pdf", "blob.bin"] {
            let mime = guess_mime(Path::new(name));
            let resp = asset_response(mime, 0, axum::body::Body::empty());
            assert_eq!(resp.headers()[header::X_CONTENT_TYPE_OPTIONS], "nosniff", "{name}");
            assert!(
                resp.headers().get(header::CONTENT_SECURITY_POLICY).is_none(),
                "{name}"
            );
        }
    }

    #[test]
    fn mime_guessing_covers_common_types_and_falls_back() {
        assert_eq!(guess_mime(Path::new("/a/b.PNG")), "image/png");
        assert_eq!(guess_mime(Path::new("/a/b.jpeg")), "image/jpeg");
        assert_eq!(guess_mime(Path::new("/a/b.svg")), "image/svg+xml");
        assert_eq!(guess_mime(Path::new("/a/b.pdf")), "application/pdf");
        assert_eq!(guess_mime(Path::new("/a/b.md")), "text/markdown; charset=utf-8");
        // Unknown / no extension → generic download type.
        assert_eq!(guess_mime(Path::new("/a/b.unknownext")), "application/octet-stream");
        assert_eq!(guess_mime(Path::new("/a/noext")), "application/octet-stream");
    }
}
