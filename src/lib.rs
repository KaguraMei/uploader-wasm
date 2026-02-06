// ============================================================================
// MinIO/S3 分片上传 WASM 模块
// ============================================================================
// 本模块实现了基于 AWS S3 V4 签名算法的分片上传功能，可用于浏览器环境
// 通过 WebAssembly 提供高性能的文件上传能力
//
// 主要功能：
// 1. 初始化分片上传 (Initiate Multipart Upload)
// 2. 上传单个分片 (Upload Part)
// 3. 完成分片上传 (Complete Multipart Upload)
//
// 使用场景：
// - 大文件上传（>5MB）
// - 需要断点续传的场景
// - 需要并行上传提升速度的场景
// ============================================================================

use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;
use web_sys::{Request, RequestInit, RequestMode, Window};
use md5::{Digest as Md5Digest}; // MD5 摘要计算（可选，用于数据完整性校验）
use sha2::{Sha256};              // SHA256 摘要计算（S3 V4 签名必需）
use hmac::{Hmac, Mac};           // HMAC 消息认证码（S3 V4 签名必需）
use js_sys::{Uint8Array, Date};  // JavaScript 互操作类型
use wasm_bindgen::JsCast;
// 定义 HMAC-SHA256 类型别名，用于 S3 V4 签名算法
type HmacSha256 = Hmac<Sha256>;



// ============================================================================
// Uploader 结构体：S3/MinIO 上传客户端
// ============================================================================
// 封装了 S3 兼容存储服务的认证信息和配置
// 建议使用 STS 临时凭证以提高安全性，避免在前端暴露长期密钥
// ============================================================================
#[wasm_bindgen]
pub struct Uploader {
    access_key: String,    // 临时访问密钥 ID (Access Key ID)
    secret_key: String,    // 临时私密访问密钥 (Secret Access Key)
    session_token: String, // STS 临时凭证的 Security Token（用于临时权限验证）
    region: String,        // 存储桶所在区域，如 "us-east-1"、"cn-north-1"
    endpoint: String,      // MinIO/S3 服务地址，如 "http://192.168.1.10:9000" 或 "https://s3.amazonaws.com"
}

#[wasm_bindgen]
impl Uploader {
    // ========================================================================
    // 构造函数：初始化 S3 客户端凭证
    // ========================================================================
    // 参数说明：
    // - ak: Access Key ID（访问密钥 ID）
    // - sk: Secret Access Key（私密访问密钥）
    // - token: Session Token（会话令牌，STS 临时凭证必需）
    // - region: 区域代码（如 "us-east-1"）
    // - endpoint: 服务端点 URL（如 "http://minio:9000"）
    //
    // 安全建议：
    // 1. 从后端 API 获取 STS 临时凭证，避免在前端硬编码长期密钥
    // 2. 设置合理的凭证过期时间（如 1 小时）
    // 3. 使用 HTTPS 传输凭证信息
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
    ) -> Result<String, JsValue> {
        let method = "PUT";

        // 提取主机名（去除协议前缀）
        // 例如：将 "http://192.168.1.10:9000" 转换为 "192.168.1.10:9000"
        // TODO: 考虑处理带端口号的情况，确保 host 不包含协议前缀
        let host = self.endpoint
            .replace("https://", "")
            .replace("http://", "");

        // 构造查询参数：对于分片上传，必须包含 partNumber 和 uploadId
        // 格式：partNumber=1&uploadId=xxx-xxx-xxx
        // TODO: 如果未来增加其他参数，必须按字母顺序对参数进行排序(S3 V4 要求)
        let query = format!("partNumber={}&uploadId={}", part_number, upload_id);

        // 获取并格式化 ISO8601 时间戳
        // S3 要求的时间格式为: YYYYMMDDTHHMMSSZ（紧凑格式，无分隔符）
        // 例如：20260206T123045Z
        let now = Date::new_0();
        let amz_date = format!(
            "{:04}{:02}{:02}T{:02}{:02}{:02}Z",
            now.get_utc_full_year(),
            now.get_utc_month() + 1,  // JavaScript 月份从 0 开始，需要 +1
            now.get_utc_date(),
            now.get_utc_hours(),
            now.get_utc_minutes(),
            now.get_utc_seconds()
        );
        let datestamp = &amz_date[..8]; // 提取日期部分: YYYYMMDD（用于签名计算）

        // 计算待上传数据的 SHA256 摘要
        // S3 V4 签名要求：必须计算 Payload 的哈希值并放入 x-amz-content-sha256 头部
        // 这样可以确保数据在传输过程中未被篡改
        let data = chunk.to_vec();
        let content_sha256 = hex::encode(Sha256::digest(&data));

        // ====================================================================
        // AWS S3 V4 签名算法实现
        // ====================================================================
        // S3 V4 签名是一个多步骤的过程，用于验证请求的合法性和完整性
        // 签名流程：规范请求 -> 待签名字符串 -> 派生密钥 -> 最终签名
        // ====================================================================

        // 步骤 1: 构造规范化资源路径 (Canonical URI)
        // 对于 MinIO Path Style 访问，路径格式为 /bucket/object
        // 例如：/my-bucket/uploads/file.zip
        // TODO: 需适配不同存储服务的路径风格（Path-Style vs Virtual-Host Style）
        let canonical_uri = format!("/{}/{}", bucket, object_key);

        // 步骤 2: 构造规范化查询字符串 (Canonical Query String)
        // 查询参数必须按字母顺序排列（本例中已经是正确顺序）
        let canonical_querystring = query.clone();

        // 步骤 3: 构造规范化请求头 (Canonical Headers)
        // 规则：
        // - 头部名称必须小写
        // - 头部必须按字母顺序排列
        // - 每个头部后面必须有换行符
        // - 格式：header-name:header-value\n
        let canonical_headers = format!(
            "host:{}\nx-amz-content-sha256:{}\nx-amz-date:{}\nx-amz-security-token:{}\n",
            host, content_sha256, amz_date, self.session_token
        );

        // 步骤 4: 列出参与签名的头部清单 (Signed Headers)
        // 必须与 canonical_headers 中的头部一致，用分号分隔
        let signed_headers = "host;x-amz-content-sha256;x-amz-date;x-amz-security-token";

        // 步骤 5: 拼接规范请求 (Canonical Request)
        // 格式：
        // HTTP方法\n
        // 规范URI\n
        // 规范查询字符串\n
        // 规范头部\n
        // 签名头部清单\n
        // Payload哈希值
        let canonical_request = format!(
            "{}\n{}\n{}\n{}\n{}\n{}",
            method, canonical_uri, canonical_querystring, canonical_headers, signed_headers, content_sha256
        );

        // 步骤 6: 构造待签名字符串 (String to Sign)
        // 格式：
        // 算法标识\n
        // 时间戳\n
        // 凭证范围\n
        // 规范请求的哈希值
        let credential_scope = format!("{}/{}/s3/aws4_request", datestamp, self.region);
        let string_to_sign = format!(
            "AWS4-HMAC-SHA256\n{}\n{}\n{}",
            amz_date,
            credential_scope,
            hex::encode(Sha256::digest(canonical_request.as_bytes()))
        );

        // 步骤 7: 计算最终签名
        // 使用派生密钥对待签名字符串进行 HMAC-SHA256 计算
        let signature = self.get_signature(datestamp, &string_to_sign);

        // 步骤 8: 构造 Authorization 头部
        // 格式：AWS4-HMAC-SHA256 Credential=<access_key>/<credential_scope>, SignedHeaders=<signed_headers>, Signature=<signature>
        let auth_header = format!(
            "AWS4-HMAC-SHA256 Credential={}/{}, SignedHeaders={}, Signature={}",
            self.access_key, credential_scope, signed_headers, signature
        );

        // ====================================================================
        // 构造并发起 HTTP 请求
        // ====================================================================

        // 初始化请求配置
        let opts = RequestInit::new();
        opts.set_method(method);
        opts.set_mode(RequestMode::Cors); // 启用跨域请求模式（CORS）
        opts.set_body(&chunk);            // 设置请求体为分片数据

        // 构造完整的请求 URL
        // 格式：http://endpoint/bucket/object?partNumber=1&uploadId=xxx
        let url = format!("{}/{}/{}?{}", self.endpoint, bucket, object_key, query);
        let request = Request::new_with_str_and_init(&url, &opts)?;

        // 设置 S3 V4 签名所需的 HTTP 头部
        let headers = request.headers();
        headers.set("x-amz-date", &amz_date)?;                      // 请求时间戳
        headers.set("x-amz-security-token", &self.session_token)?;  // STS 会话令牌
        headers.set("x-amz-content-sha256", &content_sha256)?;      // Payload 哈希值
        headers.set("Authorization", &auth_header)?;                // 签名认证信息

        // 发起 Fetch 请求（异步）
        let window: Window = web_sys::window().ok_or("no window found")?;
        let resp_value = JsFuture::from(window.fetch_with_request(&request)).await?;
        let resp: web_sys::Response = resp_value.dyn_into()?;

        // 检查响应状态码
        if !resp.ok() {
            // 上传失败，返回错误信息
            // TODO: 这里可以解析 XML 格式的错误响应以获取更详细的失败原因
            return Err(JsValue::from_str(&format!("MinIO upload failed with status: {}", resp.status())));
        }

        // 提取 ETag（用于最终合并文件）
        // ETag 是该分片的唯一标识符，格式通常为 MD5 哈希值
        // 注意：MinIO/S3 返回的 ETag 带有双引号，需要移除
        let etag = resp.headers().get("ETag")?.ok_or("ETag not found in response headers")?;
        Ok(etag.replace("\"", ""))
    }

    // ========================================================================
    // S3 V4 签名算法：计算派生密钥并生成最终签名
    // ========================================================================
    // 签名密钥派生过程（Signature Key Derivation）：
    // 1. kDate    = HMAC-SHA256("AWS4" + SecretKey, Date)
    // 2. kRegion  = HMAC-SHA256(kDate, Region)
    // 3. kService = HMAC-SHA256(kRegion, "s3")
    // 4. kSigning = HMAC-SHA256(kService, "aws4_request")
    // 5. Signature = Hex(HMAC-SHA256(kSigning, StringToSign))
    //
    // 这种多层派生的设计可以：
    // - 增强安全性（即使某一层密钥泄露，也不会直接暴露根密钥）
    // - 支持密钥缓存（同一天的请求可以复用派生密钥）
    // ========================================================================
    fn get_signature(&self, datestamp: &str, string_to_sign: &str) -> String {
        // 第 1 步：使用 "AWS4" + SecretKey 作为初始密钥，对日期进行 HMAC
        let k_date = self.hmac_sha256(format!("AWS4{}", self.secret_key).as_bytes(), datestamp.as_bytes());
        
        // 第 2 步：使用 kDate 对区域进行 HMAC
        let k_region = self.hmac_sha256(&k_date, self.region.as_bytes());
        
        // 第 3 步：使用 kRegion 对服务名称 "s3" 进行 HMAC
        let k_service = self.hmac_sha256(&k_region, b"s3");
        
        // 第 4 步：使用 kService 对 "aws4_request" 进行 HMAC，得到最终签名密钥
        let k_signing = self.hmac_sha256(&k_service, b"aws4_request");

        // 第 5 步：使用签名密钥对待签名字符串进行 HMAC，并转换为十六进制字符串
        hex::encode(self.hmac_sha256(&k_signing, string_to_sign.as_bytes()))
    }

    // ========================================================================
    // HMAC-SHA256 辅助函数
    // ========================================================================
    // 使用指定的密钥对数据进行 HMAC-SHA256 计算
    // HMAC (Hash-based Message Authentication Code) 是一种基于哈希的消息认证码
    // 可以同时验证数据的完整性和真实性
    //
    // 参数：
    // - key: HMAC 密钥（字节数组）
    // - data: 待计算的数据（字节数组）
    //
    // 返回：
    // - HMAC-SHA256 计算结果（字节数组）
    // ========================================================================
    fn hmac_sha256(&self, key: &[u8], data: &[u8]) -> Vec<u8> {
        let mut mac = HmacSha256::new_from_slice(key).expect("HMAC can take key of any size");
        mac.update(data);
        mac.finalize().into_bytes().to_vec()
    }

    // ========================================================================
    // 初始化分片上传 (Initiate Multipart Upload)
    // ========================================================================
    // 这是分片上传的第一步，向 S3/MinIO 申请一个上传会话
    // 服务器会返回一个唯一的 uploadId，用于后续的分片上传和合并操作
    //
    // 参数说明：
    // - bucket: 存储桶名称
    // - object_key: 对象键/文件路径
    //
    // 返回值：
    // - Ok(String): 上传会话 ID (uploadId)
    // - Err(JsValue): 初始化失败的错误信息
    //
    // 使用流程：
    // 1. 调用此方法获取 uploadId
    // 2. 使用 uploadId 调用 upload_part 上传各个分片
    // 3. 使用 uploadId 调用 complete_multipart_upload 完成上传
    // ========================================================================
    pub async fn initiate_multipart_upload(
        &self,
        bucket: String,
        object_key: String,
    ) -> Result<String, JsValue> {
        let method = "POST"; // HTTP 方法：POST 用于初始化分片上传
        let host = self.endpoint.replace("https://", "").replace("http://", "");
        let query = "uploads"; // S3 标准：初始化请求必须带 uploads 查询参数
        let amz_date = self.get_amz_date();
        let datestamp = &amz_date[..8];

        let canonical_uri = format!("/{}/{}", bucket, object_key);
        let canonical_querystring = query;
        
        // 空内容的 SHA256 哈希值（固定值）
        // 因为初始化请求没有 body，所以使用空字符串的 SHA256
        let content_sha256 = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

        // 计算 S3 V4 签名
        let auth_header = self.calculate_v4_auth(
            method, &canonical_uri, canonical_querystring, &amz_date, datestamp, content_sha256, &host, "host;x-amz-content-sha256;x-amz-date;x-amz-security-token"
        );

        // 构造 HTTP 请求
        let opts = RequestInit::new();
        opts.set_method(method);
        opts.set_mode(RequestMode::Cors);

        let url = format!("{}/{}/{}?{}", self.endpoint, bucket, object_key, query);
        let request = Request::new_with_str_and_init(&url, &opts)?;
        
        // 设置请求头
        let headers = request.headers();
        headers.set("x-amz-date", &amz_date)?;
        headers.set("x-amz-security-token", &self.session_token)?;
        headers.set("x-amz-content-sha256", content_sha256)?;
        headers.set("Authorization", &auth_header)?;

        // 发起请求
        let window = web_sys::window().unwrap();
        let resp_value = JsFuture::from(window.fetch_with_request(&request)).await?;
        let resp: web_sys::Response = resp_value.dyn_into()?;

        // 获取响应文本（XML 格式）
        let text = JsFuture::from(resp.text()?).await?.as_string().unwrap();

        // 从返回的 XML 中提取 UploadId
        // 响应格式示例：<InitiateMultipartUploadResult><UploadId>xxx</UploadId>...</InitiateMultipartUploadResult>
        // 使用简单字符串处理，避免引入大型 XML 解析库
        if let Some(start) = text.find("<UploadId>") {
            if let Some(end) = text.find("</UploadId>") {
                return Ok(text[start + 10..end].to_string());
            }
        }
        Err(JsValue::from_str("Failed to parse UploadId"))
    }

    // ========================================================================
    // 完成分片上传 (Complete Multipart Upload)
    // ========================================================================
    // 这是分片上传的最后一步，通知 S3/MinIO 合并所有已上传的分片
    // 服务器会按照提供的分片顺序将它们合并成最终文件
    //
    // 参数说明：
    // - bucket: 存储桶名称
    // - object_key: 对象键/文件路径
    // - upload_id: 上传会话 ID（由 initiate_multipart_upload 返回）
    // - parts_data: 所有分片的信息，格式为 "partNumber:etag,partNumber:etag,..."
    //               例如："1:abc123,2:def456,3:ghi789"
    //
    // 返回值：
    // - Ok(String): 最终文件的访问 URL
    // - Err(JsValue): 合并失败的错误信息
    //
    // 注意事项：
    // - 必须提供所有已上传分片的 ETag
    // - 分片编号必须从 1 开始连续递增
    // - ETag 必须与实际上传时返回的值一致
    // ========================================================================
    pub async fn complete_multipart_upload(
        &self,
        bucket: String,
        object_key: String,
        upload_id: String,
        parts_data: String,
    ) -> Result<String, JsValue> {
        let method = "POST"; // HTTP 方法：POST 用于完成分片上传
        let host = self.endpoint.replace("https://", "").replace("http://", "");
        let query = format!("uploadId={}", upload_id);
        let amz_date = self.get_amz_date();
        let datestamp = &amz_date[..8];

        // 构造 S3 要求的合并 XML 请求体
        // XML 格式：
        // <CompleteMultipartUpload>
        //   <Part><PartNumber>1</PartNumber><ETag>"abc123"</ETag></Part>
        //   <Part><PartNumber>2</PartNumber><ETag>"def456"</ETag></Part>
        //   ...
        // </CompleteMultipartUpload>
        let mut xml_body = String::from("<CompleteMultipartUpload>");
        for item in parts_data.split(',') {
            let p: Vec<&str> = item.split(':').collect();
            if p.len() == 2 {
                // 注意：ETag 必须用双引号包裹
                xml_body.push_str(&format!("<Part><PartNumber>{}</PartNumber><ETag>\"{}\"</ETag></Part>", p[0], p[1]));
            }
        }
        xml_body.push_str("</CompleteMultipartUpload>");

        // 计算 XML 请求体的 SHA256 哈希值
        let content_sha256 = hex::encode(Sha256::digest(xml_body.as_bytes()));

        let canonical_uri = format!("/{}/{}", bucket, object_key);
        
        // 计算 S3 V4 签名
        let auth_header = self.calculate_v4_auth(
            method, &canonical_uri, &query, &amz_date, datestamp, &content_sha256, &host, "host;x-amz-content-sha256;x-amz-date;x-amz-security-token"
        );

        // 构造 HTTP 请求
        let opts: RequestInit = RequestInit::new();
        opts.set_method(method);
        opts.set_mode(RequestMode::Cors);
        opts.set_body(&JsValue::from_str(&xml_body));

        let url = format!("{}/{}/{}?{}", self.endpoint, bucket, object_key, query);
        let request = Request::new_with_str_and_init(&url, &opts)?;
        
        // 设置请求头
        let headers = request.headers();
        headers.set("Content-Type", "application/xml")?;  // 必须指定 XML 内容类型
        headers.set("x-amz-date", &amz_date)?;
        headers.set("x-amz-security-token", &self.session_token)?;
        headers.set("x-amz-content-sha256", &content_sha256)?;
        headers.set("Authorization", &auth_header)?;

        // 发起请求
        let window = web_sys::window().unwrap();
        JsFuture::from(window.fetch_with_request(&request)).await?;

        // 返回最终文件的访问 URL
        Ok(format!("{}/{}/{}", self.endpoint, bucket, object_key))
    }

    // ========================================================================
    // 内部辅助方法：计算 S3 V4 Authorization 头部
    // ========================================================================
    // 这是一个通用的签名计算方法，被多个公开方法复用
    // 封装了 S3 V4 签名的完整流程，避免代码重复
    //
    // 参数说明：
    // - method: HTTP 方法（GET/POST/PUT/DELETE）
    // - uri: 规范化 URI（如 "/bucket/object"）
    // - query: 查询字符串（如 "uploads" 或 "uploadId=xxx"）
    // - amz_date: ISO8601 格式的时间戳
    // - datestamp: 日期部分（YYYYMMDD）
    // - content_sha256: 请求体的 SHA256 哈希值
    // - host: 主机名（不含协议）
    // - signed_headers: 参与签名的头部清单
    //
    // 返回值：
    // - Authorization 头部的完整值
    // ========================================================================
    fn calculate_v4_auth(
        &self, method: &str, uri: &str, query: &str, amz_date: &str, datestamp: &str, content_sha256: &str, host: &str, signed_headers: &str
    ) -> String {
        // 构造规范化头部
        let canonical_headers = format!(
            "host:{}\nx-amz-content-sha256:{}\nx-amz-date:{}\nx-amz-security-token:{}\n",
            host, content_sha256, amz_date, self.session_token
        );
        
        // 构造规范请求
        let canonical_request = format!(
            "{}\n{}\n{}\n{}\n{}\n{}",
            method, uri, query, canonical_headers, signed_headers, content_sha256
        );
        
        // 构造凭证范围
        let credential_scope = format!("{}/{}/s3/aws4_request", datestamp, self.region);
        
        // 构造待签名字符串
        let string_to_sign = format!(
            "AWS4-HMAC-SHA256\n{}\n{}\n{}",
            amz_date, credential_scope, hex::encode(Sha256::digest(canonical_request.as_bytes()))
        );
        
        // 计算签名
        let signature = self.get_signature(datestamp, &string_to_sign);
        
        // 返回完整的 Authorization 头部值
        format!("AWS4-HMAC-SHA256 Credential={}/{}, SignedHeaders={}, Signature={}",
                self.access_key, credential_scope, signed_headers, signature)
    }

    // ========================================================================
    // 内部辅助方法：获取当前 UTC 时间的 ISO8601 格式字符串
    // ========================================================================
    // 返回格式：YYYYMMDDTHHMMSSZ（紧凑格式，无分隔符）
    // 例如：20260206T123045Z
    // ========================================================================
    fn get_amz_date(&self) -> String {
        let now = Date::new_0();
        format!("{:04}{:02}{:02}T{:02}{:02}{:02}Z",
                now.get_utc_full_year(), 
                now.get_utc_month() + 1,  // JavaScript 月份从 0 开始
                now.get_utc_date(),
                now.get_utc_hours(), 
                now.get_utc_minutes(), 
                now.get_utc_seconds())
    }
}