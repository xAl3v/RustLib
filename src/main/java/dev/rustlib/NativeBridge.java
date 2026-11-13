package dev.rustlib;

import org.slf4j.Logger;
import org.slf4j.LoggerFactory;

import java.io.FileNotFoundException;
import java.io.IOException;
import java.io.InputStream;
import java.nio.ByteBuffer;
import java.nio.file.AtomicMoveNotSupportedException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.StandardCopyOption;
import java.security.MessageDigest;
import java.security.NoSuchAlgorithmException;
import java.util.HexFormat;
import java.util.Locale;

/**
 * JNI bridge: extracts the native library from the JAR, loads it with {@link System#load(String)} and declares
 * the {@code native} methods. This is an implementation detail: other mods should use
 * {@link RustLibAPI}.
 *
 * <p>Loading safety:
 * <ul>
 *   <li>The library is extracted to a directory named after the SHA-256 of its contents, so a different
 *       binary is never reused and an already-loaded .dll never has to be overwritten (Windows locks it).</li>
 *   <li>An already-extracted binary is re-verified by hash before it is loaded (detects corruption/tampering).</li>
 *   <li>The file is written to a temporary file and moved atomically: two game instances cannot see
 *       a half-written file.</li>
 *   <li>After loading, the ABI version is checked and a smoke-test call is made; if anything fails, the state
 *       becomes {@code FAILED} and the API never invokes native code.</li>
 * </ul>
 *
 * <p>System properties: {@code -Drustlib.disable=true} (load nothing) and
 * {@code -Drustlib.native.path=/path/to/librustlib.so} (load a specific binary, useful during development).
 */
public final class NativeBridge {
    /** Must match ABI_VERSION in native/src/lib.rs. */
    static final int EXPECTED_ABI = 1;

    private static final String LIB_NAME = "rustlib";
    private static final Logger LOG = LoggerFactory.getLogger("RustLib");

    public enum State { NOT_LOADED, LOADED, FAILED, DISABLED }

    private static volatile State state = State.NOT_LOADED;
    private static volatile String detail = "the native library has not been loaded yet";
    private static volatile int features;

    private NativeBridge() {}

    // ------------------------------------------------------------------------------------ state

    public static State state() { return state; }

    public static boolean isAvailable() { return state == State.LOADED; }

    /** Human-readable description of the state (loaded version or reason for failure). */
    public static String statusMessage() { return detail; }

    static boolean hasZstd() { return (features & 1) != 0; }

    /** Lazily loads if nobody has done so yet; throws if there is no usable native engine. */
    static void require() {
        if (state == State.NOT_LOADED) load(null);
        if (state != State.LOADED) throw new NativeUnavailableException(detail);
    }

    // ------------------------------------------------------------------------------------ loading

    /** Equivalent to {@code load(null)}: extracts to a private temporary directory. */
    public static State load() { return load(null); }

    /**
     * Detects the OS/architecture, extracts and loads the library. Idempotent and failure-proof: it does not
     * throw exceptions; check the result with {@link #state()} / {@link #statusMessage()}.
     *
     * @param cacheDir stable directory to extract into (e.g. {@code <gameDir>/.rustlib}); if
     *                 {@code null}, a private temporary directory is used.
     */
    public static synchronized State load(Path cacheDir) {
        if (state != State.NOT_LOADED) return state;

        if (Boolean.getBoolean("rustlib.disable")) {
            return set(State.DISABLED, "disabled with -Drustlib.disable=true");
        }

        String platform = detectPlatform();
        if (platform == null) {
            return fail("unsupported platform (os.name=" + System.getProperty("os.name")
                    + ", os.arch=" + System.getProperty("os.arch") + "). Supported: Windows/Linux/macOS on x86_64 and aarch64");
        }

        try {
            Path lib = resolveLibrary(platform, cacheDir);
            System.load(lib.toAbsolutePath().toString());

            int abi = nativeAbiVersion();
            if (abi != EXPECTED_ABI) {
                return fail("incompatible ABI: the JAR expects " + EXPECTED_ABI + " but the native library provides " + abi);
            }
            if (!Float.isFinite(nativeSampleNoise3D(0.5f, 0.5f, 0.5f, 0))) {
                return fail("the native library's smoke test returned a non-finite value");
            }
            features = nativeFeatures();
            String version = nativeVersion();
            set(State.LOADED, "Rust native engine v" + version + " (" + platform + (hasZstd() ? ", zstd" : "") + ")");
            LOG.info("Native library loaded: {}", detail);
            return state;
        } catch (IOException | RuntimeException | LinkageError e) {
            return fail(e.getClass().getSimpleName() + ": " + e.getMessage(), e);
        }
    }

    private static State set(State s, String message) {
        detail = message;
        state = s;
        if (s == State.DISABLED) LOG.info("RustLib {}", message);
        return s;
    }

    private static State fail(String reason) { return fail(reason, null); }

    private static State fail(String reason, Throwable cause) {
        detail = reason;
        state = State.FAILED;
        LOG.warn("Could not load the RustLib native library: {}. Accelerated functions will NOT be "
                + "available (the game continues normally; RustLibAPI.isAvailable() will return false).", reason);
        if (cause != null) LOG.debug("Native load failure details", cause);
        return state;
    }

    // ------------------------------------------------------------------------------- platform

    /** Returns e.g. "linux-x86_64", or null if unsupported. */
    static String detectPlatform() {
        String os = System.getProperty("os.name", "").toLowerCase(Locale.ROOT);
        String arch = System.getProperty("os.arch", "").toLowerCase(Locale.ROOT);

        String osId;
        if (os.contains("mac") || os.contains("darwin")) osId = "macos";
        else if (os.contains("win")) osId = "windows";
        else if (os.contains("linux")) osId = "linux";
        else return null;

        String archId = switch (arch) {
            case "amd64", "x86_64", "x64" -> "x86_64";
            case "aarch64", "arm64" -> "aarch64";
            default -> null; // includes 32-bit JVMs (x86, arm)
        };
        return archId == null ? null : osId + "-" + archId;
    }

    // ---------------------------------------------------------------------------- extraction

    private static Path resolveLibrary(String platform, Path cacheDir) throws IOException {
        String override = System.getProperty("rustlib.native.path");
        if (override != null && !override.isBlank()) {
            Path p = Path.of(override);
            if (!Files.isRegularFile(p)) throw new FileNotFoundException("rustlib.native.path does not exist: " + p);
            LOG.info("Using native library specified by -Drustlib.native.path: {}", p);
            return p;
        }

        String fileName = System.mapLibraryName(LIB_NAME);
        String resource = "/natives/" + platform + "/" + fileName;
        byte[] data;
        try (InputStream in = NativeBridge.class.getResourceAsStream(resource)) {
            if (in == null) {
                throw new FileNotFoundException("this JAR does not include the library for " + platform + " (" + resource + ")");
            }
            data = in.readAllBytes();
        }

        String sha = sha256(data);
        Path base = cacheDir != null ? cacheDir : Files.createTempDirectory("rustlib-");
        Path dir = base.resolve(sha.substring(0, 16));
        Files.createDirectories(dir);
        Path target = dir.resolve(fileName);

        if (!(Files.isRegularFile(target) && sha256(Files.readAllBytes(target)).equals(sha))) {
            Path tmp = Files.createTempFile(dir, "lib", ".tmp");
            try {
                Files.write(tmp, data);
                try {
                    Files.move(tmp, target, StandardCopyOption.ATOMIC_MOVE, StandardCopyOption.REPLACE_EXISTING);
                } catch (AtomicMoveNotSupportedException e) {
                    Files.move(tmp, target, StandardCopyOption.REPLACE_EXISTING);
                }
            } finally {
                Files.deleteIfExists(tmp);
            }
        }
        return target;
    }

    private static String sha256(byte[] data) {
        try {
            return HexFormat.of().formatHex(MessageDigest.getInstance("SHA-256").digest(data));
        } catch (NoSuchAlgorithmException e) {
            throw new IllegalStateException(e); // SHA-256 is guaranteed on every JVM
        }
    }

    // ------------------------------------------------------------------- native methods (JNI)
    //
    // Rules to avoid leaks between the JVM and Rust:
    //  * ByteBuffers MUST be direct; Rust only "borrows" them for the duration of the call and does not keep
    //    pointers. The memory is released by Java's GC/Cleaner as usual.
    //  * Rust does not return objects or create global references: results are int/float or are written
    //    into a buffer supplied by Java. Nothing to close or free manually.
    //  * Return values < 0 are error codes (see NativeAccelerationException).
    //  * A buffer must not be modified from another thread while a native call is using it.

    static native int nativeAbiVersion();

    static native int nativeFeatures();

    static native String nativeVersion();

    /** 0 = applied, 1 = the thread pool had already been created. Call before first use. */
    static native int nativeInit(int threads);

    /** native_sample_noise_3d: 3D Perlin. */
    static native float nativeSampleNoise3D(float x, float y, float z, int seed);

    static native float nativeSampleNoise(int kind, float x, float y, float z, int seed);

    static native int nativeFillNoise(int kind, ByteBuffer out, int nx, int ny, int nz,
                                      float ox, float oy, float oz, float sx, float sy, float sz,
                                      int seed, int octaves);

    /** native_raycast_aabb: 1 = hit, 0 = miss, &lt;0 = error. The result is written to {@code out}. */
    static native int nativeRaycastAabb(ByteBuffer boxes, int count,
                                        double ox, double oy, double oz,
                                        double dx, double dy, double dz,
                                        double maxDist, ByteBuffer out);

    static native int nativeRaycastVoxels(ByteBuffer grid, int sx, int sy, int sz, int gx, int gy, int gz,
                                          double ox, double oy, double oz,
                                          double dx, double dy, double dz,
                                          double maxDist, ByteBuffer out);

    static native int nativeAabbOverlaps(ByteBuffer boxes, int count,
                                         double minX, double minY, double minZ,
                                         double maxX, double maxY, double maxZ,
                                         ByteBuffer outIndices);

    static native int nativeLz4MaxSize(int len);

    static native int nativeLz4Compress(ByteBuffer src, int srcLen, ByteBuffer dst);

    static native int nativeLz4Decompress(ByteBuffer src, int srcLen, ByteBuffer dst);

    static native int nativeZstdCompress(ByteBuffer src, int srcLen, ByteBuffer dst, int level);

    static native int nativeZstdDecompress(ByteBuffer src, int srcLen, ByteBuffer dst);
}
