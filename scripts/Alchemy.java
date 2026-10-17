// Dumps what the vanilla server knows about status effects and potions but
// no report of Mojang's data generator contains: each effect's category,
// colour, attribute modifiers and particle, the effects of each potion, and
// the attributes they change (range, default, whether clients hear of them,
// a player's base value).
// Brewing recipes and enchantments are data (recipe/brewing, enchantment) and
// need no script. Run by gen-reports.sh against the unpacked server jar:
//   java -cp '<server jar>:<libraries>/*' scripts/Alchemy.java out.json
// Ferrocraft reads the result through gen-data; the server itself stays Rust.

import com.google.gson.GsonBuilder;
import com.google.gson.JsonArray;
import com.google.gson.JsonObject;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.Map;
import net.minecraft.SharedConstants;
import net.minecraft.core.Holder;
import net.minecraft.core.particles.ColorParticleOption;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.server.Bootstrap;
import net.minecraft.world.effect.InstantaneousMobEffect;
import net.minecraft.world.effect.MobEffect;
import net.minecraft.world.effect.MobEffectInstance;
import net.minecraft.world.entity.EntityTypes;
import net.minecraft.world.entity.ai.attributes.Attribute;
import net.minecraft.world.entity.ai.attributes.DefaultAttributes;
import net.minecraft.world.entity.ai.attributes.RangedAttribute;
import net.minecraft.world.item.alchemy.Potion;

public class Alchemy {
    @SuppressWarnings("unchecked")
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        var templates = MobEffect.class.getDeclaredField("attributeModifiers");
        templates.setAccessible(true);

        JsonArray effects = new JsonArray();
        for (MobEffect effect : BuiltInRegistries.MOB_EFFECT) {
            JsonObject e = new JsonObject();
            e.addProperty("name", BuiltInRegistries.MOB_EFFECT.getKey(effect).toString());
            e.addProperty("category", effect.getCategory().name().toLowerCase());
            e.addProperty("color", effect.getColor());
            e.addProperty("instant", effect.isInstantaneous());
            // The modifiers at level I; each level adds the amount once more.
            JsonArray modifiers = new JsonArray();
            var map = (Map<Holder<Attribute>, ?>) templates.get(effect);
            for (var entry : map.entrySet()) {
                var template = entry.getValue();
                var id = template.getClass().getDeclaredMethod("id");
                var amount = template.getClass().getDeclaredMethod("amount");
                var operation = template.getClass().getDeclaredMethod("operation");
                id.setAccessible(true);
                amount.setAccessible(true);
                operation.setAccessible(true);
                JsonObject m = new JsonObject();
                m.addProperty("attribute", entry.getKey().unwrapKey().orElseThrow().identifier().toString());
                m.addProperty("id", id.invoke(template).toString());
                m.addProperty("amount", (double) amount.invoke(template));
                m.addProperty("operation", operation.invoke(template).toString().toLowerCase());
                modifiers.add(m);
            }
            e.add("modifiers", modifiers);
            // The particle it shows: the coloured `entity_effect`, or its own.
            var particle = effect.createParticleOptions(new MobEffectInstance(BuiltInRegistries.MOB_EFFECT.wrapAsHolder(effect)));
            e.addProperty("particle", BuiltInRegistries.PARTICLE_TYPE.getKey(particle.getType()).toString());
            e.addProperty("colored_particle", particle instanceof ColorParticleOption);
            effects.add(e);
        }

        JsonArray potions = new JsonArray();
        for (Potion potion : BuiltInRegistries.POTION) {
            JsonObject p = new JsonObject();
            p.addProperty("name", BuiltInRegistries.POTION.getKey(potion).toString());
            // The name its translation keys use: long and strong ones share it.
            p.addProperty("base", potion.name());
            JsonArray list = new JsonArray();
            for (MobEffectInstance instance : potion.getEffects()) {
                JsonObject i = new JsonObject();
                i.addProperty("id", instance.getEffect().unwrapKey().orElseThrow().identifier().toString());
                i.addProperty("amplifier", instance.getAmplifier());
                i.addProperty("duration", instance.getDuration());
                list.add(i);
            }
            p.add("effects", list);
            potions.add(p);
        }

        JsonArray attributes = new JsonArray();
        var player = DefaultAttributes.getSupplier(EntityTypes.PLAYER);
        for (Attribute attribute : BuiltInRegistries.ATTRIBUTE) {
            var holder = BuiltInRegistries.ATTRIBUTE.wrapAsHolder(attribute);
            JsonObject a = new JsonObject();
            a.addProperty("name", BuiltInRegistries.ATTRIBUTE.getKey(attribute).toString());
            a.addProperty("default", attribute.getDefaultValue());
            if (attribute instanceof RangedAttribute ranged) {
                a.addProperty("min", ranged.getMinValue());
                a.addProperty("max", ranged.getMaxValue());
            }
            a.addProperty("sync", attribute.isClientSyncable());
            if (player.hasAttribute(holder)) {
                a.addProperty("player", player.getBaseValue(holder));
            }
            attributes.add(a);
        }

        JsonObject out = new JsonObject();
        out.add("effects", effects);
        out.add("attributes", attributes);
        out.add("potions", potions);
        Files.writeString(Path.of(args[0]), new GsonBuilder().setPrettyPrinting().create().toJson(out));
    }
}
