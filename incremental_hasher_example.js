// ============================================================================
// IncrementalHasher Usage Example
// ============================================================================
// Demonstrates how to calculate SHA256 and MD5 hashes synchronously during
// file upload process. This approach is memory-efficient for large files
// as it processes data in chunks rather than loading the entire file.
//
// Use Cases:
// - File integrity verification
// - Deduplication by content hash
// - Progress tracking during hash calculation
// - Parallel upload and hash computation
// ============================================================================

import init, { IncrementalHasher } from './pkg/uploader_wasm.js';

/**
 * Calculate SHA256 and MD5 hashes for a file using streaming approach
 * 
 * @param {File} file - The file to hash
 * @param {Object} options - Configuration options
 * @param {number} options.chunkSize - Size of each chunk in bytes (default: 1MB)
 * @param {Function} options.onProgress - Progress callback (percent: number) => void
 * @returns {Promise<{sha256: string, md5: string}>} Hash values in hexadecimal
 * 
 * @example
 * const hashes = await calculateFileHash(file, {
 *   chunkSize: 2 * 1024 * 1024, // 2MB chunks
 *   onProgress: (percent) => console.log(`Progress: ${percent}%`)
 * });
 */
async function calculateFileHash(file, options = {}) {
    // Initialize WASM module (only needs to be called once per page load)
    await init();
    
    // Create streaming hash calculator
    // This maintains internal state for both SHA256 and MD5 algorithms
    const hasher = new IncrementalHasher();
    
    // Configuration
    const CHUNK_SIZE = options.chunkSize || 1024 * 1024; // Default: 1MB
    const onProgress = options.onProgress || (() => {});
    
    let offset = 0;
    
    // Process file in chunks to avoid memory issues with large files
    while (offset < file.size) {
        // Extract next chunk from file
        const chunk = file.slice(offset, offset + CHUNK_SIZE);
        const arrayBuffer = await chunk.arrayBuffer();
        const uint8Array = new Uint8Array(arrayBuffer);
        
        // Update hash state with current chunk
        // This is where the WASM magic happens - fast native computation
        hasher.update(uint8Array);
        
        offset += CHUNK_SIZE;
        
        // Report progress
        const progress = Math.min(100, (offset / file.size) * 100);
        onProgress(progress);
        console.log(`Hash calculation progress: ${progress.toFixed(2)}%`);
    }
    
    // Finalize and retrieve hash values
    // These methods can be called multiple times without consuming the hasher
    const sha256 = hasher.finalize_sha256();
    const md5 = hasher.finalize_md5();
    
    console.log('SHA256:', sha256);
    console.log('MD5:', md5);
    
    return { sha256, md5 };
}

/**
 * Advanced example: Calculate hash while uploading
 * This demonstrates parallel hash calculation and upload for maximum efficiency
 */
async function uploadWithHash(file, uploader, bucket, objectKey) {
    await init();
    
    const hasher = new IncrementalHasher();
    const CHUNK_SIZE = 5 * 1024 * 1024; // 5MB (S3 minimum part size)
    
    // Initialize multipart upload
    const uploadId = await uploader.initiate_multipart_upload(bucket, objectKey);
    
    const uploadedParts = [];
    let offset = 0;
    let partNumber = 1;
    
    // Upload and hash simultaneously
    while (offset < file.size) {
        const chunk = file.slice(offset, offset + CHUNK_SIZE);
        const arrayBuffer = await chunk.arrayBuffer();
        const uint8Array = new Uint8Array(arrayBuffer);
        
        // Update hash (fast, synchronous)
        hasher.update(uint8Array);
        
        // Upload part (async, network I/O)
        const etag = await uploader.upload_part(
            bucket,
            objectKey,
            uploadId,
            partNumber,
            uint8Array,
            null // No abort signal for this example
        );
        
        uploadedParts.push(`${partNumber}:${etag}`);
        
        offset += CHUNK_SIZE;
        partNumber++;
        
        const progress = Math.min(100, (offset / file.size) * 100);
        console.log(`Upload & hash progress: ${progress.toFixed(2)}%`);
    }
    
    // Complete upload
    const url = await uploader.complete_multipart_upload(
        bucket,
        objectKey,
        uploadId,
        uploadedParts.join(','),
        null
    );
    
    // Get final hashes
    const hashes = {
        sha256: hasher.finalize_sha256(),
        md5: hasher.finalize_md5()
    };
    
    console.log('Upload complete:', url);
    console.log('File hashes:', hashes);
    
    return { url, hashes };
}

// ============================================================================
// Basic Usage Example: File Input Handler
// ============================================================================
const fileInput = document.querySelector('input[type="file"]');
fileInput?.addEventListener('change', async (e) => {
    const file = e.target.files[0];
    if (file) {
        console.log(`Starting hash calculation for: ${file.name}`);
        console.log(`File size: ${(file.size / 1024 / 1024).toFixed(2)} MB`);
        
        const startTime = performance.now();
        
        try {
            const hashes = await calculateFileHash(file, {
                onProgress: (percent) => {
                    // Update UI progress bar here
                    document.getElementById('progress')?.textContent = 
                        `${percent.toFixed(1)}%`;
                }
            });
            
            const duration = ((performance.now() - startTime) / 1000).toFixed(2);
            console.log(`Calculation complete in ${duration}s:`, hashes);
            
            // Display results
            document.getElementById('sha256')?.textContent = hashes.sha256;
            document.getElementById('md5')?.textContent = hashes.md5;
            
        } catch (error) {
            console.error('Hash calculation failed:', error);
        }
    }
});

// ============================================================================
// Performance Comparison: WASM vs Pure JavaScript
// ============================================================================
async function comparePerformance(file) {
    console.log('=== Performance Comparison ===');
    
    // WASM implementation
    const wasmStart = performance.now();
    const wasmHashes = await calculateFileHash(file);
    const wasmDuration = performance.now() - wasmStart;
    
    console.log(`WASM: ${wasmDuration.toFixed(2)}ms`);
    console.log('WASM SHA256:', wasmHashes.sha256);
    
    // Note: Pure JS implementation would be significantly slower
    // Typical speedup: 3-10x faster with WASM for large files
}

// Export for use in other modules
export { calculateFileHash, uploadWithHash, comparePerformance };
