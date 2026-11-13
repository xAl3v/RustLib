package dev.rustlib;

/** Error returned by the native code (a negative return code) translated into an exception. */
public final class NativeAccelerationException extends RuntimeException {
    private final int code;

    public NativeAccelerationException(String operation, int code) {
        super(operation + " failed: " + describe(code) + " (code " + code + ")");
        this.code = code;
    }

    public int code() { return code; }

    static String describe(int code) {
        return switch (code) {
            case -1 -> "invalid arguments (null/non-direct buffer, size, alignment, NaN or overlapping buffers)";
            case -2 -> "the output buffer is too small";
            case -3 -> "corrupt input data";
            case -4 -> "function not included in this native build (e.g. zstd)";
            case -99 -> "internal panic caught in Rust (the game remains stable; check stderr)";
            default -> "unknown error";
        };
    }
}
