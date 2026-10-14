// Dumps what the vanilla server knows about every block state but no report
// of Mojang's data generator contains: light, hardness, collision and block
// entities. Run by gen-reports.sh against the unpacked server jar:
//   java -cp '<server jar>:<libraries>/*' scripts/BlockPhysics.java out.json
// Ferrocraft reads the result through gen-data; the server itself stays Rust.

import com.google.gson.GsonBuilder;
import com.google.gson.JsonArray;
import com.google.gson.JsonObject;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.HashMap;
import java.util.List;
import java.util.Map;
import net.minecraft.SharedConstants;
import net.minecraft.core.BlockPos;
import net.minecraft.core.Direction;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.server.Bootstrap;
import net.minecraft.world.level.EmptyBlockGetter;
import net.minecraft.world.level.block.Block;
import net.minecraft.world.level.block.EntityBlock;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.lighting.LightEngine;
import net.minecraft.world.level.material.Fluids;
import net.minecraft.world.phys.AABB;
import net.minecraft.world.phys.shapes.VoxelShape;

public class BlockPhysics {
    /** Shapes are shared by many states; each is stored once. */
    static final class Table {
        final JsonArray entries = new JsonArray();
        final Map<String, Integer> index = new HashMap<>();

        int add(JsonArray boxes) {
            return index.computeIfAbsent(boxes.toString(), key -> {
                entries.add(boxes);
                return entries.size() - 1;
            });
        }
    }

    static double round(double v) {
        return Math.round(v * 1e6) / 1e6;
    }

    static JsonArray boxes(VoxelShape shape) {
        JsonArray boxes = new JsonArray();
        for (AABB box : shape.toAabbs()) {
            JsonArray b = new JsonArray();
            for (double v : new double[] {box.minX, box.minY, box.minZ, box.maxX, box.maxY, box.maxZ}) {
                b.add(round(v));
            }
            boxes.add(b);
        }
        return boxes;
    }

    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        var level = EmptyBlockGetter.INSTANCE;
        var pos = BlockPos.ZERO;
        Table collisions = new Table();
        Table faces = new Table();
        collisions.add(new JsonArray());
        faces.add(new JsonArray());

        JsonObject blocks = new JsonObject();
        for (Block block : BuiltInRegistries.BLOCK) {
            JsonObject out = new JsonObject();
            BlockState first = block.defaultBlockState();
            out.addProperty("item", BuiltInRegistries.ITEM.getKey(block.asItem()).toString());
            out.addProperty("hardness", first.getDestroySpeed(level, pos));
            out.addProperty("requires_tool", first.requiresCorrectToolForDrops());
            if (block instanceof EntityBlock) {
                // Asking the types avoids constructing block entities, some of
                // which need more of the game than the bootstrap sets up.
                for (var type : BuiltInRegistries.BLOCK_ENTITY_TYPE) {
                    if (type.isValid(first)) {
                        out.addProperty("block_entity", BuiltInRegistries.BLOCK_ENTITY_TYPE.getKey(type).toString());
                    }
                }
            }
            // Per state, in state id order: emission, dampening, sky light
            // passes down, collision shape, the six face occlusion shapes for
            // light (down, up, north, south, west, east), fluid.
            JsonArray states = new JsonArray();
            List<String> seen = new ArrayList<>();
            for (BlockState state : block.getStateDefinition().getPossibleStates()) {
                JsonArray s = new JsonArray();
                s.add(state.getLightEmission());
                s.add(state.getLightDampening());
                s.add(state.propagatesSkylightDown());
                s.add(collisions.add(boxes(state.getCollisionShape(level, pos))));
                for (Direction direction : Direction.values()) {
                    s.add(faces.add(boxes(LightEngine.getOcclusionShape(state, direction))));
                }
                var fluid = state.getFluidState();
                s.add(fluid.isEmpty() ? 0 : fluid.getType().isSame(Fluids.WATER) ? 1 : 2);
                states.add(s);
                seen.add(s.toString());
            }
            // Most blocks behave the same in all their states.
            if (seen.stream().distinct().count() == 1) {
                JsonArray single = new JsonArray();
                single.add(states.get(0));
                states = single;
            }
            out.add("states", states);
            blocks.add(BuiltInRegistries.BLOCK.getKey(block).toString(), out);
        }

        JsonObject root = new JsonObject();
        root.add("collision_shapes", collisions.entries);
        root.add("face_shapes", faces.entries);
        root.add("blocks", blocks);
        Files.writeString(Path.of(args[0]), new GsonBuilder().create().toJson(root));
    }
}
