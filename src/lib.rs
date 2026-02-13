// ============================================================================
// MinIO/S3 Multipart Upload WASM Module
// ============================================================================
// This module implements multipart upload functionality based on AWS S3 V4
// signature algorithm, designed for browser environments. It provides
// high-performance file upload capabilities through WebAssembly.
//
// Core Features:
// 1. Initiate Multipart Upload - Start a new upload session
// 2. Upload Part - Upload individual file chunks
// 3. Complete Multipart Upload - Finalize and merge all parts
// 4. Abort Multipart Upload - Cancel an ongoing upload session
//
// Use Cases:
// - Large file uploads (>5MB)
// - Resumable uploads with checkpoint support
// - Parallel uploads for improved throughput
// - Browser-based direct-to-S3 uploads without backend proxy
// ============================================================================

use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;
use web_sys::{Request, RequestInit, RequestMode, WorkerGlobalScope};
use md5::Md5;                    // MD5 streaming hash computation
use sha2::{Sha256, Digest};      // SHA256 digest calculation (required for S3 V4 signing)
use hmac::{Hmac, Mac};           // HMAC message authentication code (required for S3 V4 signing)
use js_sys::{Uint8Array, Date, encode_uri_component};  // JavaScript interop types
use wasm_bindgen::JsCast;

// Type alias for HMAC-SHA256, used in S3 V4 signature algorithm
type HmacSha256 = Hmac<Sha256>;

// ============================================================================
// Initialize Panic Hook: Display Rust panic messages in browser console
// ============================================================================
// This function is automatically executed when the WASM module loads.
// It captures Rust panic messages and outputs them to the browser console
// for easier debugging in web environments.
//
// Without this hook, Rust panics would be silent or show cryptic errors.
// ============================================================================
#[wasm_bindgen(start)]
pub fn init_panic_hook() {
    console_error_panic_hook::set_once();
}

// ============================================================================
// IncrementalHasher: Streaming Hash Calculator
// ============================================================================
// Computes SHA256 and MD5 hashes incrementally during file upload.
// This avoids loading the entire file into memory at once, improving
// performance for large files.
//
// Use Cases:
// - Calculate file hashes while uploading chunks
// - Verify file integrity after upload
// - Generate file fingerprints for deduplication
// - Memory-efficient hash computation for multi-GB files
//
// Implementation Notes:
// - Maintains separate state for SHA256 and MD5 algorithms
// - Can be updated with arbitrary-sized chunks
// - Finalization methods can be called multiple times (clones internal state)
// ============================================================================
#[wasm_bindgen]
pub struct IncrementalHasher {
    sha256: Sha256,
    md5_ctx: Md5,
}

#[wasm_bindgen]
impl IncrementalHasher {
    /// Create a new streaming hash calculator
    /// 
    /// Initializes both SHA256 and MD5 hash contexts.
    #[wasm_bindgen(constructor)]
    pub fn new() -> Self {
        Self {
            sha256: Sha256::new(),
            md5_ctx: Md5::new(),
        }
    }

    /// Update hash state with a new data chunk
    /// 
    /// Parameters:
    /// - chunk: JavaScript Uint8Array containing the data to hash
    /// 
    /// Notes:
    /// - Performs one memory copy from JS to Rust heap
    /// - For ~1MB chunks, the performance overhead is negligible
    /// - Safer than direct JS memory access, avoids lifetime issues
    /// - Can be called multiple times with different chunks
    /// 
    /// Example usage from JavaScript:
    /// ```js
    /// const hasher = new IncrementalHasher();
    /// const chunk1 = new Uint8Array([1, 2, 3]);
    /// const chunk2 = new Uint8Array([4, 5, 6]);
    /// hasher.update(chunk1);
    /// hasher.update(chunk2);
    /// ```
    pub fn update(&mut self, chunk: &Uint8Array) {
        // Copy JS Uint8Array to Rust Vec
        let mut buffer = vec![0u8; chunk.length() as usize];
        chunk.copy_to(&mut buffer);
        
        // Update both SHA256 and MD5 state
        self.sha256.update(&buffer);
        self.md5_ctx.update(&buffer);
    }

    /// Finalize SHA256 computation and return hexadecimal string
    /// 
    /// Returns:
    /// - SHA256 hash as lowercase hexadecimal string (64 characters)
    /// 
    /// Notes:
    /// - Clones internal state, so this method can be called multiple times
    /// - Does not consume the hasher, allowing continued updates
    pub fn finalize_sha256(&self) -> String {
        hex::encode(self.sha256.clone().finalize())
    }

    /// Finalize MD5 computation and return hexadecimal string
    /// 
    /// Returns:
    /// - MD5 hash as lowercase hexadecimal string (32 characters)
    /// 
    /// Notes:
    /// - Clones internal state, so this method can be called multiple times
    /// - Does not consume the hasher, allowing continued updates
    pub fn finalize_md5(&self) -> String {
        format!("{:x}", self.md5_ctx.clone().finalize())
    }
}



// ============================================================================
// Uploader: S3/MinIO Upload Client
// ============================================================================
// Encapsulates authentication credentials and configuration for S3-compatible
// storage services. Supports both AWS S3 and MinIO.
//
// Security Best Practices:
// - Use STS temporary credentials instead of long-term keys
// - Fetch credentials from your backend API, never hardcode in frontend
// - Set appropriate expiration times (e.g., 1 hour)
// - Use HTTPS for credential transmission
// - Implement proper CORS configuration on your S3 bucket
//
// Credential Flow:
// 1. Frontend requests temporary credentials from backend
// 2. Backend calls AWS STS AssumeRole or similar
// 3. Backend returns temporary credentials to frontend
// 4. Frontend creates Uploader with temporary credentials
// 5. Credentials expire automatically after configured duration
// ============================================================================
#[wasm_bindgen]
pub struct Uploader {
    access_key: String,    // Temporary Access Key ID
    secret_key: String,    // Temporary Secret Access Key
    session_token: String, // STS Session Token (required for temporary credentials)
    region: String,        // Bucket region (e.g., "us-east-1", "cn-north-1")
    endpoint: String,      // Service endpoint (e.g., "http://192.168.1.10:9000", "https://s3.amazonaws.com")
}

#[wasm_bindgen]
impl Uploader {
    // ========================================================================
    // Constructor: Initialize S3 client credentials
    // ========================================================================
    // Parameters:
    // - ak: Access Key ID
    // - sk: Secret Access Key
    // - token: Session Token (required for STS temporary credentials)
    // - region: AWS region code (e.g., "us-east-1", "ap-southeast-1")
    // - endpoint: Service endpoint URL (e.g., "http://minio:9000", "https://s3.amazonaws.com")
    //
    // Security Recommendations:
    // 1. Fetch STS temporary credentials from your backend API
    // 2. Never hardcode long-term credentials in frontend code
    // 3. Set reasonable credential expiration (e.g., 1 hour)
    // 4. Use HTTPS for credential transmission
    // 5. Implement proper IAM policies with least privilege
    //
    // Example JavaScript usage:
    // ```js
    // const uploader = new Uploader(
    //   "AKIAIOSFODNN7EXAMPLE",
    //   "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY",
    //   "FwoGZXIvYXdzEBYaD...",
    //   "us-east-1",
    //   "https://s3.amazonaws.com"
    // );
    // ```
    // ========================================================================
    #[wasm_bindgen(constructor)]
    pub fn new(ak: String, sk: String, token: String, region: String, endpoint: String) -> Uploader {
        Uploader {
            access_key: ak,
            secret_key: sk,
            session_token: token,
            region,
            endpoint,
        }
    }

    /// 执行分片上传（UploadPart 操作）
    /// 此方法为“黑盒”核心，内部完成：数据 SHA256 计算 -> S3 V4 签名 -> 网络请求
    pub async fn upload_part(
        &self,
        bucket: String,
        object_key: String,
        upload_id: String,
        part_number: u32,
        chunk: Uint8Array,
        signal: &JsValue,
    ) -> Result<String, JsValue> {
        // CRITICAL: Immediately copy JS data to Rust memory to avoid accessing
        // invalidated JS pointers after async await points
        let chunk_data = chunk.to_vec();

        let method = "PUT";

        // Encode upload_id to prevent special characters (. + / =) from breaking URL structure
        let encoded_upload_id = encode_uri_component(&upload_id)
            .as_string()
            .unwrap_or_else(|| upload_id.clone());

        // S3 V4 requires query parameters in alphabetical order: partNumber before uploadId
        let query = format!("partNumber={}&uploadId={}", part_number, encoded_upload_id);

        let host = self.endpoint.replace("http://", "").replace("https://", "");
        let amz_date = self.get_amz_date();
        let datestamp = &amz_date[..8];

        // Calculate SHA256 hash of the payload
        let content_sha256 = hex::encode(Sha256::digest(&chunk_data));

        // Construct canonical URI - must start with /
        // Handle object_key that may already have leading slash to prevent //
        let clean_object_key = object_key.trim_start_matches('/');
        let canonical_uri = format!("/{}/{}", bucket, clean_object_key);

        // Construct canonical headers (order matters for signature)
        let canonical_headers = format!(
            "host:{}\nx-amz-content-sha256:{}\nx-amz-date:{}\nx-amz-security-token:{}\n",
            host, content_sha256, amz_date, self.session_token
        );
        let signed_headers = "host;x-amz-content-sha256;x-amz-date;x-amz-security-token";

        let canonical_request = format!(
            "{}\n{}\n{}\n{}\n{}\n{}",
            method, canonical_uri, query, canonical_headers, signed_headers, content_sha256
        );
        let credential_scope = format!("{}/{}/s3/aws4_request", datestamp, self.region);
        let string_to_sign = format!(
            "AWS4-HMAC-SHA256\n{}\n{}\n{}",
            amz_date,
            credential_scope,
            hex::encode(Sha256::digest(canonical_request.as_bytes()))
        );

        let signature = self.get_signature(datestamp, &string_to_sign);
        let auth_header = format!(
            "AWS4-HMAC-SHA256 Credential={}/{}, SignedHeaders={}, Signature={}",
            self.access_key, credential_scope, signed_headers, signature
        );

        // Construct HTTP request
        let opts = RequestInit::new();
        opts.set_method(method);
        opts.set_mode(RequestMode::Cors);
        // Use copied Rust memory data
        let uint8_data = Uint8Array::from(&chunk_data[..]);
        opts.set_body(&uint8_data);

        // Defensive check: Set AbortSignal if provided for cancellation support
        if !signal.is_null() && !signal.is_undefined() {
            opts.set_signal(Some(signal.unchecked_ref()));
        }

        let url = format!("{}/{}/{}?{}", self.endpoint.trim_end_matches('/'), bucket, clean_object_key, query);
        let request = Request::new_with_str_and_init(&url, &opts)?;
        
        let headers = request.headers();
        headers.set("x-amz-date", &amz_date)?;
        headers.set("x-amz-security-token", &self.session_token)?;
        headers.set("x-amz-content-sha256", &content_sha256)?;
        headers.set("Authorization", &auth_header)?;

        // Send request and handle cancellation
        let resp = self.fetch_with_abort_handling(&request).await?;

        if !resp.ok() {
            let error_text = JsFuture::from(resp.text()?).await?.as_string().unwrap_or_default();
            return Err(JsValue::from_str(&format!("MinIO upload failed with status: {}, detail: {}", resp.status(), error_text)));
        }

        // Extract ETag from response headers (required for completion)
        let etag = resp.headers().get("ETag")?.ok_or("No ETag")?;
        Ok(etag.replace("\"", ""))
    }

    // ========================================================================
    // S3 V4 Signature Algorithm: Derive signing key and generate signature
    // ========================================================================
    // Signature Key Derivation Process:
    // 1. kDate    = HMAC-SHA256("AWS4" + SecretKey, Date)
    // 2. kRegion  = HMAC-SHA256(kDate, Region)
    // 3. kService = HMAC-SHA256(kRegion, "s3")
    // 4. kSigning = HMAC-SHA256(kService, "aws4_request")
    // 5. Signature = Hex(HMAC-SHA256(kSigning, StringToSign))
    //
    // This multi-layer derivation design provides:
    // - Enhanced security (even if one layer is compromised, root key remains safe)
    // - Key caching support (same-day requests can reuse derived keys)
    // - Scope isolation (different services/regions use different keys)
    // ========================================================================
    fn get_signature(&self, datestamp: &str, string_to_sign: &str) -> String {
        // Step 1: HMAC the date using "AWS4" + SecretKey as initial key
        let k_date = self.hmac_sha256(format!("AWS4{}", self.secret_key).as_bytes(), datestamp.as_bytes());
        
        // Step 2: HMAC the region using kDate
        let k_region = self.hmac_sha256(&k_date, self.region.as_bytes());
        
        // Step 3: HMAC the service name "s3" using kRegion
        let k_service = self.hmac_sha256(&k_region, b"s3");
        
        // Step 4: HMAC "aws4_request" using kService to get final signing key
        let k_signing = self.hmac_sha256(&k_service, b"aws4_request");

        // Step 5: HMAC the string-to-sign using signing key and convert to hex
        hex::encode(self.hmac_sha256(&k_signing, string_to_sign.as_bytes()))
    }

    // ========================================================================
    // HMAC-SHA256 Helper Function
    // ========================================================================
    // Computes HMAC-SHA256 using the specified key and data.
    // HMAC (Hash-based Message Authentication Code) is a cryptographic
    // algorithm that provides both data integrity and authenticity verification.
    //
    // Parameters:
    // - key: HMAC key (byte array)
    // - data: Data to compute HMAC over (byte array)
    //
    // Returns:
    // - HMAC-SHA256 result (byte array)
    //
    // Notes:
    // - HMAC can accept keys of any size
    // - Used extensively in S3 V4 signature derivation
    // - Provides cryptographic strength for authentication
    // ========================================================================
    fn hmac_sha256(&self, key: &[u8], data: &[u8]) -> Vec<u8> {
        let mut mac = HmacSha256::new_from_slice(key).expect("HMAC can take key of any size");
        mac.update(data);
        mac.finalize().into_bytes().to_vec()
    }

    // ========================================================================
    // Initiate Multipart Upload
    // ========================================================================
    // This is the first step of multipart upload. It requests an upload session
    // from S3/MinIO. The server returns a unique uploadId for subsequent
    // part uploads and completion.
    //
    // Parameters:
    // - bucket: Bucket name
    // - object_key: Object key/file path
    //
    // Returns:
    // - Ok(String): Upload session ID (uploadId)
    // - Err(JsValue): Initialization error message
    //
    // Workflow:
    // 1. Call this method to obtain uploadId
    // 2. Use uploadId to call upload_part for each chunk
    // 3. Use uploadId to call complete_multipart_upload to finalize
    //
    // Notes:
    // - The uploadId is valid until explicitly completed or aborted
    // - Incomplete uploads may incur storage costs
    // - Consider implementing automatic cleanup for abandoned uploads
    // ========================================================================
    pub async fn initiate_multipart_upload(
        &self,
        bucket: String,
        object_key: String,
    ) -> Result<String, JsValue> {
        let method = "POST"; // HTTP method: POST for initiating multipart upload
        
        // Normalize query string: for key-only parameters, must append '='
        // URL uses ?uploads, signature uses uploads=
        let canonical_querystring = "uploads=";
        let query_for_url = "uploads";
        
        let host = self.endpoint.replace("http://", "").replace("https://", "");
        let amz_date = self.get_amz_date();
        let datestamp = &amz_date[..8];

        // Empty payload for initialization, SHA256 is a fixed constant
        let content_sha256 = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

        // Ensure proper URI encoding (standard practice even for clean filenames)
        let canonical_uri = format!("/{}/{}", bucket, object_key);

        // Construct canonical request
        let canonical_headers = format!(
            "host:{}\nx-amz-content-sha256:{}\nx-amz-date:{}\nx-amz-security-token:{}\n",
            host, content_sha256, amz_date, self.session_token
        );
        let signed_headers = "host;x-amz-content-sha256;x-amz-date;x-amz-security-token";

        let canonical_request = format!(
            "{}\n{}\n{}\n{}\n{}\n{}",
            method, canonical_uri, canonical_querystring, canonical_headers, signed_headers, content_sha256
        );

        let credential_scope = format!("{}/{}/s3/aws4_request", datestamp, self.region);
        let string_to_sign = format!(
            "AWS4-HMAC-SHA256\n{}\n{}\n{}",
            amz_date,
            credential_scope,
            hex::encode(Sha256::digest(canonical_request.as_bytes()))
        );

        let signature = self.get_signature(datestamp, &string_to_sign);
        let auth_header = format!(
            "AWS4-HMAC-SHA256 Credential={}/{}, SignedHeaders={}, Signature={}",
            self.access_key, credential_scope, signed_headers, signature
        );

        // Construct and send HTTP request
        let opts = RequestInit::new();
        opts.set_method(method);
        opts.set_mode(RequestMode::Cors);

        // URL uses original ?uploads format
        let url = format!("{}/{}/{}?{}", self.endpoint.trim_end_matches('/'), bucket, object_key, query_for_url);
        let request = Request::new_with_str_and_init(&url, &opts)?;
        
        let headers = request.headers();
        headers.set("x-amz-date", &amz_date)?;
        headers.set("x-amz-security-token", &self.session_token)?;
        headers.set("x-amz-content-sha256", content_sha256)?;
        headers.set("Authorization", &auth_header)?;

        let resp = self.fetch_with_abort_handling(&request).await?;

        if !resp.ok() {
            let error_text = JsFuture::from(resp.text()?).await?.as_string().unwrap_or_default();
            return Err(JsValue::from_str(&format!("MinIO Error ({}): {}", resp.status(), error_text)));
        }

        let text = JsFuture::from(resp.text()?).await?.as_string().unwrap_or_default();
        
        // Extract UploadId from XML response
        if let Some(start_idx) = text.find("<UploadId>") {
            if let Some(end_idx) = text.find("</UploadId>") {
                return Ok(text[start_idx + 10..end_idx].to_string());
            }
        }
        Err(JsValue::from_str(&format!("UploadId not found: {}", text)))
    }

    // ========================================================================
    // Complete Multipart Upload
    // ========================================================================
    // This is the final step of multipart upload. It instructs S3/MinIO to
    // merge all uploaded parts. The server will combine them in the provided
    // order to create the final file.
    //
    // Parameters:
    // - bucket: Bucket name
    // - object_key: Object key/file path
    // - upload_id: Upload session ID (returned by initiate_multipart_upload)
    // - parts_data: All part information in format "partNumber:etag,partNumber:etag,..."
    //               Example: "1:abc123,2:def456,3:ghi789"
    // - signal: AbortSignal for cancellation support
    //
    // Returns:
    // - Ok(String): Final file access URL
    // - Err(JsValue): Merge failure error message
    //
    // Important Notes:
    // - Must provide ETags for all uploaded parts
    // - Part numbers must start from 1 and be sequential
    // - ETags must match the values returned during upload
    // - Parts will be merged in the order specified
    // - Missing or incorrect ETags will cause the operation to fail
    // ========================================================================
    pub async fn complete_multipart_upload(
        &self,
        bucket: String,
        object_key: String,
        upload_id: String,
        parts_data: String,
        signal: &JsValue,
    ) -> Result<String, JsValue> {
        let method = "POST"; // HTTP method: POST for completing multipart upload
        let host = self.endpoint.replace("https://", "").replace("http://", "");
        let query = format!("uploadId={}", upload_id);
        let amz_date = self.get_amz_date();
        let datestamp = &amz_date[..8];

        // Construct S3-required merge XML request body
        // XML format:
        // <CompleteMultipartUpload>
        //   <Part><PartNumber>1</PartNumber><ETag>"abc123"</ETag></Part>
        //   <Part><PartNumber>2</PartNumber><ETag>"def456"</ETag></Part>
        //   ...
        // </CompleteMultipartUpload>
        let mut xml_body = String::from("<CompleteMultipartUpload>");
        for item in parts_data.split(',') {
            let p: Vec<&str> = item.split(':').collect();
            if p.len() == 2 {
                // Note: ETag must be wrapped in double quotes
                xml_body.push_str(&format!("<Part><PartNumber>{}</PartNumber><ETag>\"{}\"</ETag></Part>", p[0], p[1]));
            }
        }
        xml_body.push_str("</CompleteMultipartUpload>");

        // Calculate SHA256 hash of XML request body
        let content_sha256 = hex::encode(Sha256::digest(xml_body.as_bytes()));

        let canonical_uri = format!("/{}/{}", bucket, object_key);
        
        // Calculate S3 V4 signature
        let auth_header = self.calculate_v4_auth(
            method, &canonical_uri, &query, &amz_date, datestamp, &content_sha256, &host, "host;x-amz-content-sha256;x-amz-date;x-amz-security-token"
        );

        // Construct HTTP request
        let opts: RequestInit = RequestInit::new();
        // Defensive check: Set AbortSignal if provided for cancellation support
        if !signal.is_null() && !signal.is_undefined() {
            opts.set_signal(Some(signal.unchecked_ref()));
        }
        opts.set_method(method);
        opts.set_mode(RequestMode::Cors);
        opts.set_body(&JsValue::from_str(&xml_body));

        let url = format!("{}/{}/{}?{}", self.endpoint, bucket, object_key, query);
        let request = Request::new_with_str_and_init(&url, &opts)?;
        
        // Set request headers
        let headers = request.headers();
        headers.set("Content-Type", "application/xml")?;  // Must specify XML content type
        headers.set("x-amz-date", &amz_date)?;
        headers.set("x-amz-security-token", &self.session_token)?;
        headers.set("x-amz-content-sha256", &content_sha256)?;
        headers.set("Authorization", &auth_header)?;

        // Send request and handle cancellation
        let resp = self.fetch_with_abort_handling(&request).await?;

        // Check response status code
        if !resp.ok() {
            let error_text = JsFuture::from(resp.text()?)
                .await?
                .as_string()
                .unwrap_or_default();
            return Err(JsValue::from_str(&format!(
                "Complete multipart upload failed ({}): {}",
                resp.status(),
                error_text
            )));
        }

        // Return final file access URL
        Ok(format!("{}/{}/{}", self.endpoint, bucket, object_key))
    }

    // ========================================================================
    // Internal Helper: Calculate S3 V4 Authorization Header
    // ========================================================================
    // This is a generic signature calculation method reused by multiple
    // public methods. It encapsulates the complete S3 V4 signing process
    // to avoid code duplication.
    //
    // Parameters:
    // - method: HTTP method (GET/POST/PUT/DELETE)
    // - uri: Canonical URI (e.g., "/bucket/object")
    // - query: Query string (e.g., "uploads" or "uploadId=xxx")
    // - amz_date: ISO8601 timestamp
    // - datestamp: Date portion (YYYYMMDD)
    // - content_sha256: SHA256 hash of request body
    // - host: Hostname (without protocol)
    // - signed_headers: List of headers included in signature
    //
    // Returns:
    // - Complete Authorization header value
    //
    // Notes:
    // - Follows AWS Signature Version 4 specification
    // - Headers must be in canonical form (lowercase, sorted)
    // - Query parameters must be URL-encoded and sorted
    // ========================================================================
    fn calculate_v4_auth(
        &self, method: &str, uri: &str, query: &str, amz_date: &str, datestamp: &str, content_sha256: &str, host: &str, signed_headers: &str
    ) -> String {
        // Construct canonical headers
        let canonical_headers = format!(
            "host:{}\nx-amz-content-sha256:{}\nx-amz-date:{}\nx-amz-security-token:{}\n",
            host, content_sha256, amz_date, self.session_token
        );
        
        // Construct canonical request
        let canonical_request = format!(
            "{}\n{}\n{}\n{}\n{}\n{}",
            method, uri, query, canonical_headers, signed_headers, content_sha256
        );
        
        // Construct credential scope
        let credential_scope = format!("{}/{}/s3/aws4_request", datestamp, self.region);
        
        // Construct string to sign
        let string_to_sign = format!(
            "AWS4-HMAC-SHA256\n{}\n{}\n{}",
            amz_date, credential_scope, hex::encode(Sha256::digest(canonical_request.as_bytes()))
        );
        
        // Calculate signature
        let signature = self.get_signature(datestamp, &string_to_sign);
        
        // Return complete Authorization header value
        format!("AWS4-HMAC-SHA256 Credential={}/{}, SignedHeaders={}, Signature={}",
                self.access_key, credential_scope, signed_headers, signature)
    }

    // ========================================================================
    // Internal Helper: Get Current UTC Time in ISO8601 Format
    // ========================================================================
    // Returns compact ISO8601 format without separators: YYYYMMDDTHHMMSSZ
    // Example: 20260206T123045Z
    //
    // This format is required by AWS Signature Version 4 specification.
    // The timestamp must be in UTC timezone (indicated by 'Z' suffix).
    // ========================================================================
    fn get_amz_date(&self) -> String {
        let now = Date::new_0();
        format!("{:04}{:02}{:02}T{:02}{:02}{:02}Z",
                now.get_utc_full_year(), 
                now.get_utc_month() + 1,  // JavaScript months are 0-indexed
                now.get_utc_date(),
                now.get_utc_hours(), 
                now.get_utc_minutes(), 
                now.get_utc_seconds())
    }

    // ========================================================================
    // Internal Helper: Execute HTTP Request with Abort Handling
    // ========================================================================
    // Unified fetch request handler that automatically distinguishes between
    // user cancellation and network errors.
    // 
    // Parameters:
    // - request: web_sys::Request object
    // 
    // Returns:
    // - Ok(Response): Successful response object
    // - Err("USER_CANCELED"): User actively canceled the request
    // - Err(other): Network error or other exception
    //
    // Notes:
    // - Detects AbortError from AbortSignal and converts to "USER_CANCELED"
    // - Works in both Window and Worker contexts
    // - Allows caller to distinguish cancellation from failure
    // ========================================================================
    async fn fetch_with_abort_handling(&self, request: &Request) -> Result<web_sys::Response, JsValue> {
        // Inner helper function: Handle fetch errors
        fn handle_fetch_error(e: JsValue) -> Result<JsValue, JsValue> {
            if let Some(dom_err) = e.dyn_ref::<web_sys::DomException>() {
                if dom_err.name() == "AbortError" {
                    return Err(JsValue::from_str("USER_CANCELED"));
                }
            }
            Err(e)
        }

        let global = js_sys::global();
        
        // Try Window context first, fallback to Worker context
        let resp_value = if let Some(window) = web_sys::window() {
            JsFuture::from(window.fetch_with_request(request))
                .await
                .or_else(handle_fetch_error)?
        } else {
            let worker_global = global.unchecked_into::<WorkerGlobalScope>();
            JsFuture::from(worker_global.fetch_with_request(request))
                .await
                .or_else(handle_fetch_error)?
        };
        
        resp_value.dyn_into()
    }

    // ========================================================================
    // Abort Multipart Upload
    // ========================================================================
    // Cancels an ongoing multipart upload session and releases storage space
    // occupied by uploaded parts on the server. This is important for handling
    // upload failures, user cancellations, and preventing storage costs.
    //
    // Parameters:
    // - bucket: Bucket name
    // - object_key: Object key/file path
    // - upload_id: Upload session ID (returned by initiate_multipart_upload)
    //
    // Returns:
    // - Ok(()): Successfully aborted upload
    // - Err(JsValue): Abort failure error message
    //
    // Important Notes:
    // - After abortion, the uploadId becomes invalid and cannot be reused
    // - All uploaded parts will be deleted and cannot be recovered
    // - Recommended to call this on upload failure or user cancellation
    // - Prevents incurring storage costs for incomplete uploads
    // - S3/MinIO may have automatic cleanup policies for abandoned uploads
    // ========================================================================
    pub async fn abort_multipart_upload(
        &self,
        bucket: String,
        object_key: String,
        upload_id: String,
    ) -> Result<(), JsValue> {
        let method = "DELETE";
        let host = self.endpoint.replace("https://", "").replace("http://", "");
        let amz_date = self.get_amz_date();
        let datestamp = &amz_date[..8];
        
        // Encode upload_id to handle special characters
        let encoded_upload_id = encode_uri_component(&upload_id)
            .as_string()
            .unwrap_or(upload_id);
        let query = format!("uploadId={}", encoded_upload_id);

        // DELETE requests typically have no body, SHA256 is empty hash constant
        let content_sha256 = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
        let canonical_uri = format!("/{}/{}", bucket, object_key);

        let auth_header = self.calculate_v4_auth(
            method,
            &canonical_uri,
            &query,
            &amz_date,
            datestamp,
            content_sha256,
            &host,
            "host;x-amz-content-sha256;x-amz-date;x-amz-security-token"
        );

        let opts = RequestInit::new();
        opts.set_method(method);
        opts.set_mode(RequestMode::Cors);

        let url = format!("{}/{}/{}?{}", self.endpoint.trim_end_matches('/'), bucket, object_key, query);
        let request = Request::new_with_str_and_init(&url, &opts)?;
        
        let headers = request.headers();
        headers.set("x-amz-date", &amz_date)?;
        headers.set("x-amz-security-token", &self.session_token)?;
        headers.set("x-amz-content-sha256", content_sha256)?;
        headers.set("Authorization", &auth_header)?;

        let resp = self.fetch_with_abort_handling(&request).await?;

        if !resp.ok() {
            return Err(JsValue::from_str("Abort multipart upload failed"));
        }

        Ok(())
    }
}