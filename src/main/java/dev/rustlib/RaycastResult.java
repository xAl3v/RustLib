package dev.rustlib;

/**
 * Result of a raycast. If {@code hit} is false, the remaining fields are not meaningful.
 *
 * @param boxIndex index of the box that was hit (AABB raycast) or -1 (voxel raycast)
 * @param distance distance along the ray's normalized direction
 * @param voxelX   world coordinates of the voxel that was hit (voxel raycast only)
 */
public record RaycastResult(boolean hit, Face face, int boxIndex, double distance,
                            double x, double y, double z,
                            int voxelX, int voxelY, int voxelZ) {

    /** Face through which the ray enters the box/voxel. */
    public enum Face {
        NEG_X, POS_X, NEG_Y, POS_Y, NEG_Z, POS_Z,
        /** The ray origin was already inside the box or a solid voxel (distance 0). */
        INSIDE,
        NONE;

        static Face fromCode(int code) {
            return code >= 0 && code < NONE.ordinal() ? values()[code] : NONE;
        }
    }

    public static final RaycastResult MISS =
            new RaycastResult(false, Face.NONE, -1, Double.POSITIVE_INFINITY, 0, 0, 0, 0, 0, 0);
}
