package dev.rustlib;

/**
 * Thrown when {@link RustLibAPI} is used but the native engine could not be loaded (unsupported
 * platform, missing binary, incompatible ABI...). If your mod must also work without acceleration, check
 * {@link RustLibAPI#isAvailable()} first.
 */
public final class NativeUnavailableException extends IllegalStateException {
    public NativeUnavailableException(String reason) {
        super("RustLib native engine unavailable: " + reason);
    }
}
