# AbortSignal Usage Guide

## Overview

This guide explains how to implement cancellable uploads using the `AbortSignal` API with the WASM uploader module. The implementation provides fine-grained control over network requests, allowing users to cancel ongoing uploads gracefully.

## Implementation Summary

The WASM module has been enhanced with the following safety and cancellation features:

1. ✅ Added `DomException` support to `web-sys` features for proper error handling
2. ✅ Added `signal` parameter to `upload_part` and `complete_multipart_upload` methods
3. ✅ Implemented defensive checks (`is_null()` and `is_undefined()`) for all network requests
4. ✅ Unified `AbortError` handling that returns `"USER_CANCELED"` identifier for easy error discrimination

## Why AbortSignal?

- **User Experience**: Allow users to cancel long-running uploads
- **Resource Management**: Free up network bandwidth and browser resources
- **Cost Control**: Prevent unnecessary S3/MinIO storage costs from incomplete uploads
- **Error Recovery**: Cleanly handle network interruptions and timeouts

## JavaScript Usage Examples

### 1. Basic Usage: Cancel a Single Part Upload

```javascript
// Create an AbortController
const controller = new AbortController();

// Upload a part with cancellation support
uploader
  .upload_part(
    bucket,
    objectKey,
    uploadId,
    partNumber,
    chunkData,
    controller.signal, // Pass the signal
  )
  .then((etag) => {
    console.log("Upload successful:", etag);
  })
  .catch((err) => {
    if (err === "USER_CANCELED") {
      console.log("User canceled the upload");
    } else {
      console.error("Upload failed:", err);
    }
  });

// When user clicks cancel button
cancelButton.onclick = () => {
  controller.abort(); // Immediately abort the request
};
```

### 2. Advanced Usage: Cancel Multiple Parts in Batch

```javascript
// One controller can control multiple requests
const controller = new AbortController();
const signal = controller.signal;

// Upload 6 parts in parallel
const uploadPromises = chunks.map((chunk, index) => {
  return uploader.upload_part(
    bucket,
    objectKey,
    uploadId,
    index + 1,
    chunk,
    signal, // All parts share the same signal
  );
});

// Cancel all uploads with one click
cancelAllButton.onclick = () => {
  controller.abort(); // All 6 requests stop simultaneously
};

// Handle results
Promise.allSettled(uploadPromises).then((results) => {
  results.forEach((result, index) => {
    if (result.status === "rejected" && result.reason === "USER_CANCELED") {
      console.log(`Part ${index + 1} was canceled by user`);
    } else if (result.status === "fulfilled") {
      console.log(`Part ${index + 1} uploaded successfully:`, result.value);
    } else {
      console.error(`Part ${index + 1} upload failed:`, result.reason);
    }
  });
});
```

### 3. Complete Upload Flow with Cancellation Support

```javascript
class UploadManager {
  constructor(uploader) {
    this.uploader = uploader;
    this.controller = null;
    this.uploadId = null;
  }

  async uploadFile(file, bucket, objectKey) {
    // Create a new controller for this upload session
    this.controller = new AbortController();
    const signal = this.controller.signal;

    try {
      // Step 1: Initialize multipart upload
      this.uploadId = await this.uploader.initiate_multipart_upload(
        bucket,
        objectKey,
      );
      console.log("Upload session started:", this.uploadId);

      // Step 2: Upload parts
      const chunkSize = 5 * 1024 * 1024; // 5MB (S3 minimum)
      const chunks = this.splitFile(file, chunkSize);
      const uploadedParts = [];

      for (let i = 0; i < chunks.length; i++) {
        try {
          console.log(`Uploading part ${i + 1}/${chunks.length}...`);

          const etag = await this.uploader.upload_part(
            bucket,
            objectKey,
            this.uploadId,
            i + 1,
            chunks[i],
            signal, // Pass signal for cancellation
          );

          uploadedParts.push(`${i + 1}:${etag}`);

          // Update progress
          const progress = ((i + 1) / chunks.length) * 100;
          this.onProgress?.(progress);
        } catch (err) {
          if (err === "USER_CANCELED") {
            // User canceled - clean up server resources
            console.log("Upload canceled, cleaning up...");
            await this.uploader.abort_multipart_upload(
              bucket,
              objectKey,
              this.uploadId,
            );
            throw new Error("Upload canceled by user");
          }
          throw err;
        }
      }

      // Step 3: Complete the upload
      console.log("Completing multipart upload...");
      const result = await this.uploader.complete_multipart_upload(
        bucket,
        objectKey,
        this.uploadId,
        uploadedParts.join(","),
        signal, // Complete also supports cancellation
      );

      console.log("Upload completed successfully:", result);
      return result;
    } catch (err) {
      console.error("Upload failed:", err);
      throw err;
    } finally {
      this.controller = null;
      this.uploadId = null;
    }
  }

  cancel() {
    if (this.controller) {
      console.log("Canceling upload...");
      this.controller.abort();
    }
  }

  splitFile(file, chunkSize) {
    const chunks = [];
    for (let i = 0; i < file.size; i += chunkSize) {
      const end = Math.min(i + chunkSize, file.size);
      chunks.push(file.slice(i, end));
    }
    return chunks;
  }

  // Optional: Set progress callback
  setProgressCallback(callback) {
    this.onProgress = callback;
  }
}

// Usage example
const manager = new UploadManager(uploader);

// Set up progress tracking
manager.setProgressCallback((percent) => {
  progressBar.style.width = `${percent}%`;
  progressText.textContent = `${percent.toFixed(1)}%`;
});

// Start upload
uploadButton.onclick = async () => {
  try {
    uploadButton.disabled = true;
    cancelButton.disabled = false;

    const url = await manager.uploadFile(file, "my-bucket", "path/to/file.dat");
    console.log("Upload complete:", url);
    alert("Upload successful!");
  } catch (err) {
    console.error("Upload error:", err);
    alert(`Upload failed: ${err.message}`);
  } finally {
    uploadButton.disabled = false;
    cancelButton.disabled = true;
  }
};

// Cancel upload
cancelButton.onclick = () => {
  manager.cancel();
};
```

### 4. Timeout Implementation

```javascript
/**
 * Upload with automatic timeout
 * Cancels the upload if it takes longer than specified duration
 */
async function uploadWithTimeout(
  uploader,
  bucket,
  key,
  uploadId,
  part,
  chunk,
  timeoutMs = 30000,
) {
  const controller = new AbortController();

  // Set up timeout
  const timeoutId = setTimeout(() => {
    console.log("Upload timeout, aborting...");
    controller.abort();
  }, timeoutMs);

  try {
    const etag = await uploader.upload_part(
      bucket,
      key,
      uploadId,
      part,
      chunk,
      controller.signal,
    );

    clearTimeout(timeoutId);
    return etag;
  } catch (err) {
    clearTimeout(timeoutId);

    if (err === "USER_CANCELED") {
      throw new Error("Upload timeout exceeded");
    }
    throw err;
  }
}
```

### 5. Retry Logic with Cancellation

```javascript
/**
 * Upload with automatic retry on failure
 * Still respects user cancellation
 */
async function uploadWithRetry(
  uploader,
  bucket,
  key,
  uploadId,
  part,
  chunk,
  signal,
  maxRetries = 3,
) {
  let lastError;

  for (let attempt = 1; attempt <= maxRetries; attempt++) {
    try {
      console.log(`Upload attempt ${attempt}/${maxRetries}...`);

      const etag = await uploader.upload_part(
        bucket,
        key,
        uploadId,
        part,
        chunk,
        signal,
      );

      return etag;
    } catch (err) {
      lastError = err;

      // Don't retry if user canceled
      if (err === "USER_CANCELED") {
        throw err;
      }

      // Don't retry on last attempt
      if (attempt === maxRetries) {
        break;
      }

      // Exponential backoff
      const delay = Math.min(1000 * Math.pow(2, attempt - 1), 10000);
      console.log(`Retry in ${delay}ms...`);
      await new Promise((resolve) => setTimeout(resolve, delay));
    }
  }

  throw new Error(`Upload failed after ${maxRetries} attempts: ${lastError}`);
}
```

## Error Handling Best Practices

### Distinguish Error Types

```javascript
try {
  await uploader.upload_part(bucket, key, uploadId, 1, chunk, signal);
} catch (err) {
  if (err === "USER_CANCELED") {
    // User actively canceled - normal flow, no error notification needed
    updateUI("Upload canceled");
    logEvent("upload_canceled", { reason: "user_action" });
  } else if (typeof err === "string" && err.includes("MinIO upload failed")) {
    // Server error - may need retry
    console.error("Server error:", err);

    // Extract status code if available
    const statusMatch = err.match(/status: (\d+)/);
    const status = statusMatch ? parseInt(statusMatch[1]) : null;

    if (status === 503 || status === 429) {
      // Service unavailable or rate limited - retry with backoff
      await retryWithBackoff();
    } else {
      showErrorDialog("Server error occurred. Please try again.");
    }
  } else if (
    err.message?.includes("NetworkError") ||
    err.message?.includes("Failed to fetch")
  ) {
    // Network error - check connection
    console.error("Network error:", err);
    showErrorDialog(
      "Network connection lost. Please check your internet connection.",
    );
  } else {
    // Unknown error
    console.error("Unknown error:", err);
    showErrorDialog(`An unexpected error occurred: ${err}`);
  }
}
```

### Comprehensive Error Handler

```javascript
class UploadErrorHandler {
  static handle(error, context = {}) {
    const errorInfo = {
      type: this.categorizeError(error),
      message: error.toString(),
      context,
      timestamp: new Date().toISOString(),
    };

    // Log for analytics
    this.logError(errorInfo);

    // User notification
    switch (errorInfo.type) {
      case "USER_CANCELED":
        return { shouldRetry: false, userMessage: "Upload canceled" };

      case "NETWORK_ERROR":
        return { shouldRetry: true, userMessage: "Network error. Retrying..." };

      case "SERVER_ERROR":
        return { shouldRetry: true, userMessage: "Server error. Retrying..." };

      case "TIMEOUT":
        return {
          shouldRetry: true,
          userMessage: "Request timeout. Retrying...",
        };

      default:
        return {
          shouldRetry: false,
          userMessage: "Upload failed. Please try again.",
        };
    }
  }

  static categorizeError(error) {
    if (error === "USER_CANCELED") return "USER_CANCELED";

    const errorStr = error.toString().toLowerCase();

    if (errorStr.includes("network") || errorStr.includes("fetch")) {
      return "NETWORK_ERROR";
    }
    if (errorStr.includes("timeout")) {
      return "TIMEOUT";
    }
    if (errorStr.includes("minio") || errorStr.includes("status")) {
      return "SERVER_ERROR";
    }

    return "UNKNOWN";
  }

  static logError(errorInfo) {
    // Send to analytics service
    console.error("[Upload Error]", errorInfo);

    // Could send to Sentry, LogRocket, etc.
    // Sentry.captureException(errorInfo);
  }
}

// Usage
try {
  await uploader.upload_part(bucket, key, uploadId, 1, chunk, signal);
} catch (err) {
  const { shouldRetry, userMessage } = UploadErrorHandler.handle(err, {
    bucket,
    key,
    partNumber: 1,
  });

  showNotification(userMessage);

  if (shouldRetry) {
    await retryUpload();
  }
}
```

## Important Considerations

### 1. Signal Lifecycle Management

```javascript
// ❌ WRONG: Controller released too early
function badExample() {
  const controller = new AbortController();
  uploader.upload_part(bucket, key, uploadId, 1, chunk, controller.signal);
  // controller is destroyed here, but request is still in progress
  // This can lead to memory leaks or unexpected behavior
}

// ✅ CORRECT: Keep controller reference until request completes
class GoodExample {
  constructor() {
    this.controller = new AbortController();
  }

  async upload() {
    try {
      await uploader.upload_part(
        bucket,
        key,
        uploadId,
        1,
        chunk,
        this.controller.signal,
      );
    } finally {
      // Clean up after request completes
      this.controller = null;
    }
  }

  cancel() {
    if (this.controller) {
      this.controller.abort();
    }
  }
}
```

### 2. Handling null/undefined Signals

```javascript
// WASM module has defensive checks - all of these are safe:
await uploader.upload_part(bucket, key, uploadId, 1, chunk, null);
await uploader.upload_part(bucket, key, uploadId, 1, chunk, undefined);

// If you don't need cancellation, you can pass a never-aborted signal
const neverAbort = new AbortController().signal;
await uploader.upload_part(bucket, key, uploadId, 1, chunk, neverAbort);

// Or simply pass null for clarity
await uploader.upload_part(bucket, key, uploadId, 1, chunk, null);
```

### 3. Server-Side Cleanup

```javascript
/**
 * Always clean up server resources when canceling
 * This prevents storage costs from incomplete uploads
 */
async function uploadWithCleanup(uploader, file, bucket, key) {
  const controller = new AbortController();
  let uploadId = null;

  try {
    // Initialize upload
    uploadId = await uploader.initiate_multipart_upload(bucket, key);

    // Upload parts
    const chunks = splitFile(file);
    for (let i = 0; i < chunks.length; i++) {
      await uploader.upload_part(
        bucket,
        key,
        uploadId,
        i + 1,
        chunks[i],
        controller.signal,
      );
    }

    // Complete upload
    await uploader.complete_multipart_upload(
      bucket,
      key,
      uploadId,
      partsData,
      controller.signal,
    );
  } catch (err) {
    // Clean up on any error, including cancellation
    if (uploadId) {
      try {
        await uploader.abort_multipart_upload(bucket, key, uploadId);
        console.log("Server resources cleaned up");
      } catch (abortErr) {
        console.error("Failed to clean up:", abortErr);
      }
    }
    throw err;
  }
}
```

### 4. Multiple Upload Sessions

```javascript
/**
 * Manage multiple concurrent uploads
 * Each upload has its own controller
 */
class MultiUploadManager {
  constructor(uploader) {
    this.uploader = uploader;
    this.uploads = new Map(); // uploadId -> controller
  }

  async startUpload(file, bucket, key) {
    const controller = new AbortController();

    try {
      const uploadId = await this.uploader.initiate_multipart_upload(
        bucket,
        key,
      );

      // Store controller for this upload session
      this.uploads.set(uploadId, { controller, bucket, key, file });

      // Start upload process
      await this.processUpload(uploadId, file, bucket, key, controller.signal);

      return uploadId;
    } catch (err) {
      console.error("Upload failed:", err);
      throw err;
    }
  }

  cancelUpload(uploadId) {
    const upload = this.uploads.get(uploadId);
    if (upload) {
      upload.controller.abort();
      console.log(`Canceled upload: ${uploadId}`);
    }
  }

  cancelAll() {
    console.log(`Canceling ${this.uploads.size} uploads...`);
    for (const [uploadId, upload] of this.uploads) {
      upload.controller.abort();
    }
    this.uploads.clear();
  }

  async processUpload(uploadId, file, bucket, key, signal) {
    // Implementation details...
  }
}
```

## Performance Considerations

### 1. Zero-Cost Abstraction

The WASM implementation uses `unchecked_ref` for signal handling, which provides:

- **Zero runtime overhead**: Direct JS object reference without type checking
- **Minimal defensive checks**: `is_null()` and `is_undefined()` have negligible cost
- **Native performance**: No serialization or copying of signal objects

### 2. Network Cancellation Immediacy

When `abort()` is called:

- Browser immediately terminates the TCP connection
- No data is sent or received after cancellation
- Resources are freed instantly
- WASM module receives `AbortError` synchronously

### 3. Memory Efficiency

```javascript
// Good: Reuse controller for sequential uploads
const controller = new AbortController();

for (const file of files) {
  try {
    await uploadFile(file, controller.signal);
  } catch (err) {
    if (err === "USER_CANCELED") break;
  }
}

// Better: Create new controller for each upload
// This prevents accidental cancellation of subsequent uploads
for (const file of files) {
  const controller = new AbortController();
  try {
    await uploadFile(file, controller.signal);
  } catch (err) {
    if (err === "USER_CANCELED") {
      console.log(`Skipped: ${file.name}`);
      continue; // Continue with next file
    }
  }
}
```

## Debugging Tips

### 1. Monitor Signal State

```javascript
const controller = new AbortController();
const signal = controller.signal;

// Listen for abort event
signal.addEventListener("abort", () => {
  console.log("Signal aborted at:", new Date().toISOString());
  console.trace("Abort stack trace");
});

// Check signal status
console.log("Is aborted:", signal.aborted);

// Abort and verify
controller.abort();
console.log("Is aborted:", signal.aborted); // true
```

### 2. Debug Upload Flow

```javascript
class DebugUploadManager extends UploadManager {
  async upload_part(...args) {
    const [bucket, key, uploadId, partNumber, chunk, signal] = args;

    console.log(`[Part ${partNumber}] Starting upload...`);
    console.log(`[Part ${partNumber}] Signal aborted:`, signal?.aborted);
    console.log(`[Part ${partNumber}] Chunk size:`, chunk.length);

    const startTime = performance.now();

    try {
      const etag = await super.upload_part(...args);
      const duration = performance.now() - startTime;

      console.log(`[Part ${partNumber}] Success in ${duration.toFixed(2)}ms`);
      console.log(`[Part ${partNumber}] ETag:`, etag);

      return etag;
    } catch (err) {
      const duration = performance.now() - startTime;

      console.error(
        `[Part ${partNumber}] Failed after ${duration.toFixed(2)}ms`,
      );
      console.error(`[Part ${partNumber}] Error:`, err);

      throw err;
    }
  }
}
```

### 3. Test Cancellation Scenarios

```javascript
/**
 * Test suite for cancellation behavior
 */
async function testCancellation() {
  console.log("=== Testing Cancellation ===");

  // Test 1: Cancel before upload starts
  {
    const controller = new AbortController();
    controller.abort(); // Abort immediately

    try {
      await uploader.upload_part(
        bucket,
        key,
        uploadId,
        1,
        chunk,
        controller.signal,
      );
      console.error("Test 1 FAILED: Should have thrown");
    } catch (err) {
      console.log("Test 1 PASSED:", err === "USER_CANCELED");
    }
  }

  // Test 2: Cancel during upload
  {
    const controller = new AbortController();

    const uploadPromise = uploader.upload_part(
      bucket,
      key,
      uploadId,
      1,
      chunk,
      controller.signal,
    );

    // Cancel after 100ms
    setTimeout(() => controller.abort(), 100);

    try {
      await uploadPromise;
      console.error("Test 2 FAILED: Should have thrown");
    } catch (err) {
      console.log("Test 2 PASSED:", err === "USER_CANCELED");
    }
  }

  // Test 3: Multiple parts with shared signal
  {
    const controller = new AbortController();

    const promises = [1, 2, 3].map((i) =>
      uploader.upload_part(bucket, key, uploadId, i, chunk, controller.signal),
    );

    setTimeout(() => controller.abort(), 50);

    const results = await Promise.allSettled(promises);
    const allCanceled = results.every(
      (r) => r.status === "rejected" && r.reason === "USER_CANCELED",
    );

    console.log("Test 3 PASSED:", allCanceled);
  }

  console.log("=== Tests Complete ===");
}
```

## Browser Compatibility

The `AbortSignal` API is supported in:

- Chrome 66+
- Firefox 57+
- Safari 12.1+
- Edge 16+

For older browsers, consider using a polyfill:

```javascript
// Check for AbortController support
if (typeof AbortController === "undefined") {
  console.warn("AbortController not supported, loading polyfill...");
  await import("abortcontroller-polyfill");
}
```

## Additional Resources

- [MDN: AbortController](https://developer.mozilla.org/en-US/docs/Web/API/AbortController)
- [MDN: AbortSignal](https://developer.mozilla.org/en-US/docs/Web/API/AbortSignal)
- [AWS S3 Multipart Upload](https://docs.aws.amazon.com/AmazonS3/latest/userguide/mpuoverview.html)
- [Rust wasm-bindgen Guide](https://rustwasm.github.io/wasm-bindgen/)

## Summary

The AbortSignal integration provides:

- ✅ Graceful cancellation of uploads
- ✅ Resource cleanup on both client and server
- ✅ Fine-grained control over individual or batch operations
- ✅ Zero-cost abstraction with native performance
- ✅ Comprehensive error handling and debugging support

Use this feature to build robust, user-friendly upload experiences that respect user control and system resources.
