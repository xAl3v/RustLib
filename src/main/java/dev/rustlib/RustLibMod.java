package dev.rustlib;

import net.fabricmc.api.ModInitializer;
import net.fabricmc.loader.api.FabricLoader;
import org.slf4j.Logger;
import org.slf4j.LoggerFactory;

import java.nio.file.Path;

/**
 * Fabric entrypoint. Loads the native engine when the game starts; if that fails, it leaves a clear warning
 * in the log and the game continues (dependent mods can query {@link RustLibAPI#isAvailable()}).
 */
public final class RustLibMod implements ModInitializer {
    private static final Logger LOG = LoggerFactory.getLogger("RustLib");

    @Override
    public void onInitialize() {
        Path cache = FabricLoader.getInstance().getGameDir().resolve(".rustlib");
        NativeBridge.State state = NativeBridge.load(cache);
        if (state == NativeBridge.State.LOADED) {
            LOG.info("RustLib ready: {}", NativeBridge.statusMessage());
        } else {
            LOG.warn("RustLib running in degraded mode ({}): {}", state, NativeBridge.statusMessage());
        }
    }
}
