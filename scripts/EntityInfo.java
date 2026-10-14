// Dumps what the vanilla server knows about every entity type but no report
// of Mojang's data generator contains: size, eye height, how far and how
// often clients are told about it, its category and, for living entities,
// the base values of their attributes. Run by gen-reports.sh against the
// unpacked server jar:
//   java -cp '<server jar>:<libraries>/*' scripts/EntityInfo.java out.json
// Ferrocraft reads the result through gen-data; the server itself stays Rust.

import com.google.gson.GsonBuilder;
import com.google.gson.JsonObject;
import java.nio.file.Files;
import java.nio.file.Path;
import net.minecraft.SharedConstants;
import net.minecraft.core.Holder;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.server.Bootstrap;
import net.minecraft.world.entity.EntityType;
import net.minecraft.world.entity.LivingEntity;
import net.minecraft.world.entity.ai.attributes.Attribute;
import net.minecraft.world.entity.ai.attributes.DefaultAttributes;

public class EntityInfo {
    @SuppressWarnings("unchecked")
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        JsonObject entities = new JsonObject();
        for (EntityType<?> type : BuiltInRegistries.ENTITY_TYPE) {
            JsonObject out = new JsonObject();
            var dimensions = type.getDimensions();
            out.addProperty("width", dimensions.width());
            out.addProperty("height", dimensions.height());
            out.addProperty("eye_height", dimensions.eyeHeight());
            // In chunks, as `ChunkMap` multiplies it by 16.
            out.addProperty("tracking_range", type.clientTrackingRange());
            out.addProperty("update_interval", type.updateInterval());
            out.addProperty("category", type.getCategory().getName());
            if (DefaultAttributes.hasSupplier(type)) {
                var supplier = DefaultAttributes.getSupplier((EntityType<? extends LivingEntity>) type);
                JsonObject attributes = new JsonObject();
                for (Holder<Attribute> attribute : BuiltInRegistries.ATTRIBUTE.asHolderIdMap()) {
                    if (supplier.hasAttribute(attribute)) {
                        String name = attribute.unwrapKey().orElseThrow().identifier().getPath();
                        attributes.addProperty(name, supplier.getBaseValue(attribute));
                    }
                }
                out.add("attributes", attributes);
            }
            entities.add(BuiltInRegistries.ENTITY_TYPE.getKey(type).toString(), out);
        }
        Files.writeString(Path.of(args[0]), new GsonBuilder().create().toJson(entities));
    }
}
