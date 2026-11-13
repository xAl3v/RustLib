package dev.rustlib;

import java.nio.ByteBuffer;
import java.nio.ByteOrder;
import java.util.Objects;

/**
 * Public API for offloading heavy computations to the native engine (Rust). This is the only class other mods should use.
 *
 * <pre>{@code
 * if (RustLibAPI.isAvailable()) {
 *     ByteBuffer heights = RustLibAPI.allocateFloats(16 * 16 * 16);
 *     RustLibAPI.fillNoise(NoiseType.SIMPLEX_3D, heights, 16, 16, 16,
 *             chunkX * 16f, 0f, chunkZ * 16f, 0.05f, 0.05f, 0.05f, seed, 4);
 *     float v = heights.getFloat(0); // data is read from off-heap memory without copying
 * }
 * }</pre>
 *
 * <p><b>Buffer rules:</b> buffers must be <i>direct</i> and, if they hold floats/doubles, use native
 * byte order ({@link #allocateFloats} and {@link #allocateAabbs} take care of this for you). The engine uses the
 * buffer's capacity and base address (use {@code slice()} for sub-views) and ignores {@code position}/{@code limit}.
 * Do not modify a buffer from another thread while a call is using it. The methods are thread-safe
 * and can be called from any thread.
 *
 * <p>If the native engine is not available, the methods throw {@link NativeUnavailableException}.
 */
public final class RustLibAPI {
    private RustLibAPI() {}

    /** Bytes per AABB in box buffers: 6 doubles (minX, minY, minZ, maxX, maxY, maxZ). */
    public static final int AABB_BYTES = 48;
    private static final int RESULT_BYTES = 64;

    private static final ThreadLocal<ByteBuffer> SCRATCH =
            ThreadLocal.withInitial(() -> ByteBuffer.allocateDirect(RESULT_BYTES).order(ByteOrder.nativeOrder()));

    public enum NoiseType {
        /** Improved 3D Perlin, range [-1, 1]. SIMD path in {@link #fillNoise}. */
        PERLIN_3D(0),
        /** 2D Simplex (ignores z), range [-1, 1]. */
        SIMPLEX_2D(1),
        /** 3D Simplex, range [-1, 1]. */
        SIMPLEX_3D(2),
        /** 3D Worley/cellular (F1 distance to the nearest point), range [0, 1]. */
        WORLEY_3D(3);

        final int id;

        NoiseType(int id) { this.id = id; }
    }

    // ------------------------------------------------------------------------------------ state

    /** Is the native engine loaded and usable? Loads lazily if loading has not been attempted yet. */
    public static boolean isAvailable() {
        if (NativeBridge.state() == NativeBridge.State.NOT_LOADED) NativeBridge.load();
        return NativeBridge.isAvailable();
    }

    /** Text with the loaded version or the reason why the engine is unavailable. */
    public static String status() { return NativeBridge.statusMessage(); }

    public static boolean isZstdAvailable() { return isAvailable() && NativeBridge.hasZstd(); }

    /**
     * Sets the number of threads in the native pool (0 = automatic: half of the cores). Only works before
     * the first parallel computation; returns false if the pool already existed.
     */
    public static boolean setThreadCount(int threads) {
        NativeBridge.require();
        return NativeBridge.nativeInit(Math.max(0, threads)) == 0;
    }

    // ------------------------------------------------------------------------------------- buffers

    /** Direct buffer in native byte order for {@code count} floats. */
    public static ByteBuffer allocateFloats(int count) {
        return ByteBuffer.allocateDirect(Math.multiplyExact(count, 4)).order(ByteOrder.nativeOrder());
    }

    /** Direct buffer in native byte order for {@code count} AABBs (6 doubles each). */
    public static ByteBuffer allocateAabbs(int count) {
        return ByteBuffer.allocateDirect(Math.multiplyExact(count, AABB_BYTES)).order(ByteOrder.nativeOrder());
    }

    /** Writes box number {@code index} into a buffer created with {@link #allocateAabbs}. */
    public static void putAabb(ByteBuffer boxes, int index,
                               double minX, double minY, double minZ, double maxX, double maxY, double maxZ) {
        int o = index * AABB_BYTES;
        boxes.putDouble(o, minX).putDouble(o + 8, minY).putDouble(o + 16, minZ)
                .putDouble(o + 24, maxX).putDouble(o + 32, maxY).putDouble(o + 40, maxZ);
    }

    // -------------------------------------------------------------------------------------- noise

    /** 3D Perlin at a single point (native_sample_noise_3d). For many points use {@link #fillNoise}. */
    public static float sampleNoise3D(float x, float y, float z, int seed) {
        NativeBridge.require();
        return NativeBridge.nativeSampleNoise3D(x, y, z, seed);
    }

    public static float sampleNoise(NoiseType type, float x, float y, float z, int seed) {
        NativeBridge.require();
        return NativeBridge.nativeSampleNoise(type.id, x, y, z, seed);
    }

    /**
     * Fills {@code out} with fBm noise. Layout: {@code index = x + sizeX * (y + sizeY * z)}.
     * The sampling coordinate of cell (i,j,k) is {@code origin + (i,j,k) * step}.
     *
     * @param octaves 1..8 (lacunarity 2, gain 0.5)
     */
    public static void fillNoise(NoiseType type, ByteBuffer out, int sizeX, int sizeY, int sizeZ,
                                 float originX, float originY, float originZ,
                                 float stepX, float stepY, float stepZ, int seed, int octaves) {
        Objects.requireNonNull(type, "type");
        checkDirect(out, "out");
        checkNativeOrder(out, "out");
        if (sizeX <= 0 || sizeY <= 0 || sizeZ <= 0) throw new IllegalArgumentException("dimensions must be > 0");
        long need = 4L * sizeX * sizeY * sizeZ;
        if (need > out.capacity()) {
            throw new IllegalArgumentException("out needs " + need + " bytes but only has " + out.capacity());
        }
        NativeBridge.require();
        check(NativeBridge.nativeFillNoise(type.id, out, sizeX, sizeY, sizeZ,
                originX, originY, originZ, stepX, stepY, stepZ, seed, octaves), "fillNoise");
    }

    // ------------------------------------------------------------------------- collisions / raycast

    /**
     * Ray against {@code count} AABBs (native_raycast_aabb). The direction does not need to be normalized.
     * Returns the nearest box at a distance &lt;= {@code maxDistance}.
     */
    public static RaycastResult raycastAabb(ByteBuffer boxes, int count,
                                            double ox, double oy, double oz,
                                            double dx, double dy, double dz, double maxDistance) {
        checkDirect(boxes, "boxes");
        checkNativeOrder(boxes, "boxes");
        if (count < 0 || (long) count * AABB_BYTES > boxes.capacity()) {
            throw new IllegalArgumentException("count is out of range for the capacity of boxes");
        }
        NativeBridge.require();
        ByteBuffer out = SCRATCH.get();
        int rc = check(NativeBridge.nativeRaycastAabb(boxes, count, ox, oy, oz, dx, dy, dz, maxDistance, out), "raycastAabb");
        return rc == 0 ? RaycastResult.MISS : parse(out);
    }

    /**
     * Amanatides-Woo raycast over a voxel grid: 1 byte per voxel (!= 0 = solid), index
     * {@code (y * sizeZ + z) * sizeX + x}. {@code gridX/Y/Z} is the world coordinate of voxel (0,0,0).
     */
    public static RaycastResult raycastVoxels(ByteBuffer grid, int sizeX, int sizeY, int sizeZ,
                                              int gridX, int gridY, int gridZ,
                                              double ox, double oy, double oz,
                                              double dx, double dy, double dz, double maxDistance) {
        checkDirect(grid, "grid");
        if (sizeX <= 0 || sizeY <= 0 || sizeZ <= 0 || (long) sizeX * sizeY * sizeZ > grid.capacity()) {
            throw new IllegalArgumentException("invalid dimensions for the capacity of grid");
        }
        NativeBridge.require();
        ByteBuffer out = SCRATCH.get();
        int rc = check(NativeBridge.nativeRaycastVoxels(grid, sizeX, sizeY, sizeZ, gridX, gridY, gridZ,
                ox, oy, oz, dx, dy, dz, maxDistance, out), "raycastVoxels");
        return rc == 0 ? RaycastResult.MISS : parse(out);
    }

    /**
     * Writes into {@code outIndices} (ints in native byte order) the indices of the boxes that overlap the
     * query box (strict overlap). Returns the TOTAL number of overlaps; if it exceeds
     * {@code outIndices.capacity() / 4}, the list was truncated.
     */
    public static int findOverlappingAabbs(ByteBuffer boxes, int count,
                                           double minX, double minY, double minZ,
                                           double maxX, double maxY, double maxZ, ByteBuffer outIndices) {
        checkDirect(boxes, "boxes");
        checkNativeOrder(boxes, "boxes");
        checkDirect(outIndices, "outIndices");
        checkNativeOrder(outIndices, "outIndices");
        if (count < 0 || (long) count * AABB_BYTES > boxes.capacity()) {
            throw new IllegalArgumentException("count is out of range for the capacity of boxes");
        }
        NativeBridge.require();
        return check(NativeBridge.nativeAabbOverlaps(boxes, count, minX, minY, minZ, maxX, maxY, maxZ, outIndices),
                "findOverlappingAabbs");
    }

    // -------------------------------------------------------------------------------- compression

    /** Buffer size sufficient to compress {@code inputLength} bytes with LZ4. */
    public static int lz4MaxCompressedSize(int inputLength) {
        NativeBridge.require();
        return check(NativeBridge.nativeLz4MaxSize(inputLength), "lz4MaxCompressedSize");
    }

    /** Compresses {@code srcLength} bytes from {@code src} into {@code dst}; returns the bytes written. */
    public static int lz4Compress(ByteBuffer src, int srcLength, ByteBuffer dst) {
        checkPair(src, srcLength, dst);
        NativeBridge.require();
        return check(NativeBridge.nativeLz4Compress(src, srcLength, dst), "lz4Compress");
    }

    /** Decompresses an LZ4 block; {@code dst} must be able to hold the original size. Returns the bytes written. */
    public static int lz4Decompress(ByteBuffer src, int srcLength, ByteBuffer dst) {
        checkPair(src, srcLength, dst);
        NativeBridge.require();
        return check(NativeBridge.nativeLz4Decompress(src, srcLength, dst), "lz4Decompress");
    }

    /** Requires a native binary built with the {@code zstd} feature ({@link #isZstdAvailable()}). */
    public static int zstdCompress(ByteBuffer src, int srcLength, ByteBuffer dst, int level) {
        checkPair(src, srcLength, dst);
        NativeBridge.require();
        return check(NativeBridge.nativeZstdCompress(src, srcLength, dst, level), "zstdCompress");
    }

    public static int zstdDecompress(ByteBuffer src, int srcLength, ByteBuffer dst) {
        checkPair(src, srcLength, dst);
        NativeBridge.require();
        return check(NativeBridge.nativeZstdDecompress(src, srcLength, dst), "zstdDecompress");
    }

    // ----------------------------------------------------------------------------------- helpers

    private static RaycastResult parse(ByteBuffer r) {
        return new RaycastResult(true, RaycastResult.Face.fromCode(r.getInt(4)), r.getInt(8), r.getDouble(16),
                r.getDouble(24), r.getDouble(32), r.getDouble(40), r.getInt(48), r.getInt(52), r.getInt(56));
    }

    private static int check(int code, String operation) {
        if (code < 0) throw new NativeAccelerationException(operation, code);
        return code;
    }

    private static void checkDirect(ByteBuffer b, String name) {
        Objects.requireNonNull(b, name);
        if (!b.isDirect()) {
            throw new IllegalArgumentException(name + " must be a direct ByteBuffer (ByteBuffer.allocateDirect)");
        }
    }

    private static void checkNativeOrder(ByteBuffer b, String name) {
        if (b.order() != ByteOrder.nativeOrder()) {
            throw new IllegalArgumentException(name + " must use ByteOrder.nativeOrder()");
        }
    }

    private static void checkPair(ByteBuffer src, int srcLength, ByteBuffer dst) {
        checkDirect(src, "src");
        checkDirect(dst, "dst");
        if (srcLength < 0 || srcLength > src.capacity()) throw new IllegalArgumentException("srcLength is out of range");
    }
}
