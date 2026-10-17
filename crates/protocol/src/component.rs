//! Item components (`DataComponents`): how a value travels on the wire
//! (`STREAM_CODEC`) and how vanilla saves it (`CODEC`), described once as a
//! [`Shape`] for both.
//!
//! A value is held as the NBT vanilla saves: a record is a compound without
//! the optional fields that are absent or at their default, a registry entry
//! is its name, an enum value its lower-case name. Reading from the wire
//! gives that form, writing to the wire takes it; registry entries are
//! looked up through [`Registries`]. Components without a shape here stay in
//! their wire form (see `ComponentPatch`).

use std::sync::OnceLock;

use crate::codec::{Decode, Encode, Error, Result, VarInt, decode_count, decode_string};
use crate::nbt::Nbt;

/// Component type names by network id, for reading plain slots.
static TYPE_NAMES: OnceLock<Vec<String>> = OnceLock::new();

/// Tells the protocol the `minecraft:data_component_type` registry, so
/// that a slot as the server sends it can be read: its component values
/// carry no length, only their shape tells where they end.
pub fn set_type_names(names: Vec<String>) {
    let _ = TYPE_NAMES.set(names);
}

/// The name of a component type by network id, if known.
pub fn type_name(id: i32) -> Option<&'static str> {
    TYPE_NAMES
        .get()?
        .get(usize::try_from(id).ok()?)
        .map(String::as_str)
}

/// Network ids of registry entries, which only the server's data knows.
pub trait Registries {
    /// The name of entry `id`, e.g. `minecraft:sharpness` in
    /// `minecraft:enchantment`.
    fn name(&self, registry: &str, id: i32) -> Option<String>;
    fn id(&self, registry: &str, name: &str) -> Option<i32>;
}

/// How a value is written.
#[derive(Debug)]
pub enum Shape {
    VarInt,
    Int,
    Float,
    /// A byte on the wire, `0b`/`1b` saved.
    Bool,
    String,
    /// An NBT tag as it is: text components, custom data.
    Tag,
    /// Nothing on the wire, `{}` saved.
    Unit,
    /// A registry entry: its network id on the wire, its name saved.
    Holder(&'static str),
    /// An enum: its id on the wire, its name saved.
    Enum(&'static [&'static str]),
    /// A count, then the elements.
    List(&'static Shape),
    /// A count, then key and value of each entry; saved as a compound keyed
    /// by the key's name.
    Map(&'static Shape, &'static Shape),
    /// The fields in order on the wire; saved as a compound.
    Record(&'static [Field]),
    /// Another shape, for records that hold themselves.
    Lazy(fn() -> &'static Shape),
}

/// A field of a [`Shape::Record`].
#[derive(Debug)]
pub struct Field {
    pub name: &'static str,
    pub shape: Shape,
    pub presence: Presence,
}

#[derive(Debug)]
pub enum Presence {
    /// Always there.
    Required,
    /// A `bool` on the wire says whether it follows; left out when saved
    /// (`ByteBufCodecs.optional`, `optionalFieldOf`).
    Optional,
    /// Always on the wire; saved only when it differs from this
    /// (`optionalFieldOf(name, default)`).
    Default(Nbt),
    /// Always on the wire and saved; this if a saved value lacks it.
    Fallback(Nbt),
}

const fn field(name: &'static str, shape: Shape) -> Field {
    Field {
        name,
        shape,
        presence: Presence::Required,
    }
}

const fn optional(name: &'static str, shape: Shape) -> Field {
    Field {
        name,
        shape,
        presence: Presence::Optional,
    }
}

const fn fallback(name: &'static str, shape: Shape, default: Nbt) -> Field {
    Field {
        name,
        shape,
        presence: Presence::Fallback(default),
    }
}

const fn defaulted(name: &'static str, shape: Shape, default: Nbt) -> Field {
    Field {
        name,
        shape,
        presence: Presence::Default(default),
    }
}

/// `Rarity`.
pub const RARITIES: &[&str] = &["common", "uncommon", "rare", "epic"];
/// `DyeColor`, by id.
pub const DYE_COLORS: &[&str] = &[
    "white",
    "orange",
    "magenta",
    "light_blue",
    "yellow",
    "lime",
    "pink",
    "gray",
    "light_gray",
    "cyan",
    "purple",
    "blue",
    "brown",
    "green",
    "red",
    "black",
];

/// `MobEffectInstance.Details`.
static EFFECT_DETAILS: [Field; 6] = [
    defaulted("amplifier", Shape::VarInt, Nbt::Int(0)),
    defaulted("duration", Shape::VarInt, Nbt::Int(0)),
    defaulted("ambient", Shape::Bool, Nbt::Byte(0)),
    defaulted("show_particles", Shape::Bool, Nbt::Byte(1)),
    fallback("show_icon", Shape::Bool, Nbt::Byte(1)),
    optional("hidden_effect", Shape::Lazy(effect_details)),
];

fn effect_details() -> &'static Shape {
    static DETAILS: Shape = Shape::Record(&EFFECT_DETAILS);
    &DETAILS
}

/// `MobEffectInstance`: the effect, then its details, in one compound.
pub static EFFECT_INSTANCE: Shape = Shape::Record(&[
    field("id", Shape::Holder("minecraft:mob_effect")),
    defaulted("amplifier", Shape::VarInt, Nbt::Int(0)),
    defaulted("duration", Shape::VarInt, Nbt::Int(0)),
    defaulted("ambient", Shape::Bool, Nbt::Byte(0)),
    defaulted("show_particles", Shape::Bool, Nbt::Byte(1)),
    fallback("show_icon", Shape::Bool, Nbt::Byte(1)),
    optional("hidden_effect", Shape::Lazy(effect_details)),
]);

static ENCHANTMENT: Shape = Shape::Holder("minecraft:enchantment");
static LEVEL: Shape = Shape::VarInt;
static TEXT: Shape = Shape::Tag;
static STEW_EFFECT: Shape = Shape::Record(&[
    field("id", Shape::Holder("minecraft:mob_effect")),
    defaulted("duration", Shape::VarInt, Nbt::Int(160)),
]);
static COMPONENT_TYPE: Shape = Shape::Holder("minecraft:data_component_type");

/// `PotionContents`.
static POTION_CONTENTS: Shape = Shape::Record(&[
    optional("potion", Shape::Holder("minecraft:potion")),
    optional("custom_color", Shape::Int),
    defaulted(
        "custom_effects",
        Shape::List(&EFFECT_INSTANCE),
        Nbt::List(Vec::new()),
    ),
    optional("custom_name", Shape::String),
]);

/// A component type with a shape.
#[derive(Debug)]
pub struct Component {
    pub name: &'static str,
    pub shape: &'static Shape,
    /// Another saved form vanilla also reads (`Codec.withAlternative`),
    /// turned into the main one.
    pub alternative: Option<fn(&Nbt) -> Option<Nbt>>,
}

macro_rules! components {
    ($($name:literal => $shape:expr $(, or $alt:expr)?;)*) => {
        &[$(Component {
            name: $name,
            shape: { static SHAPE: Shape = $shape; &SHAPE },
            alternative: components!(@alt $($alt)?),
        }),*]
    };
    (@alt) => { None };
    (@alt $alt:expr) => { Some($alt) };
}

/// The components Ferrocraft understands, by name.
pub static COMPONENTS: &[Component] = components! {
    "minecraft:custom_data" => Shape::Tag;
    "minecraft:max_stack_size" => Shape::VarInt;
    "minecraft:max_damage" => Shape::VarInt;
    "minecraft:damage" => Shape::VarInt;
    "minecraft:unbreakable" => Shape::Unit;
    "minecraft:custom_name" => Shape::Tag;
    "minecraft:minimum_attack_charge" => Shape::Float;
    "minecraft:item_name" => Shape::Tag;
    "minecraft:item_model" => Shape::String;
    "minecraft:lore" => Shape::List(&TEXT);
    "minecraft:rarity" => Shape::Enum(RARITIES);
    "minecraft:enchantments" => Shape::Map(&ENCHANTMENT, &LEVEL);
    "minecraft:tooltip_display" => Shape::Record(&[
        defaulted("hide_tooltip", Shape::Bool, Nbt::Byte(0)),
        defaulted("hidden_components", Shape::List(&COMPONENT_TYPE), Nbt::List(Vec::new())),
    ]);
    "minecraft:repair_cost" => Shape::VarInt;
    "minecraft:creative_slot_lock" => Shape::Unit;
    "minecraft:enchantment_glint_override" => Shape::Bool;
    "minecraft:enchantable" => Shape::Record(&[field("value", Shape::VarInt)]);
    "minecraft:glider" => Shape::Unit;
    "minecraft:tooltip_style" => Shape::String;
    "minecraft:stored_enchantments" => Shape::Map(&ENCHANTMENT, &LEVEL);
    "minecraft:dye" => Shape::Enum(DYE_COLORS);
    "minecraft:dyed_color" => Shape::Int;
    "minecraft:map_id" => Shape::VarInt;
    "minecraft:potion_contents" => Shape::Lazy(|| &POTION_CONTENTS), or potion_by_name;
    "minecraft:potion_duration_scale" => Shape::Float;
    "minecraft:suspicious_stew_effects" => Shape::List(&STEW_EFFECT);
    "minecraft:ominous_bottle_amplifier" => Shape::VarInt;
    "minecraft:note_block_sound" => Shape::String;
    "minecraft:base_color" => Shape::Enum(DYE_COLORS);
    "minecraft:waxed" => Shape::Unit;
};

/// `potion_contents` saved as only the potion's name.
fn potion_by_name(saved: &Nbt) -> Option<Nbt> {
    match saved {
        Nbt::String(name) => Some(Nbt::Compound(vec![(
            "potion".into(),
            Nbt::String(name.clone()),
        )])),
        _ => None,
    }
}

/// The component called `name`, if it has a shape here.
pub fn component(name: &str) -> Option<&'static Component> {
    COMPONENTS.iter().find(|c| c.name == name)
}

impl Component {
    /// A saved value (from a file or a data pack) in its wire form, after
    /// checking it fits.
    pub fn to_wire(&self, saved: &Nbt, registries: &dyn Registries) -> Result<Vec<u8>> {
        let mut out = Vec::new();
        match self.shape.write(saved, &mut out, registries) {
            Ok(()) => Ok(out),
            Err(err) => {
                let Some(other) = self.alternative.and_then(|alt| alt(saved)) else {
                    return Err(err);
                };
                out.clear();
                self.shape.write(&other, &mut out, registries)?;
                Ok(out)
            }
        }
    }

    /// The saved form of a value on the wire; all of `bytes` must be used.
    pub fn from_wire(&self, mut bytes: &[u8], registries: &dyn Registries) -> Result<Nbt> {
        let value = self.shape.read(&mut bytes, registries)?;
        if !bytes.is_empty() {
            return Err(Error::Invalid("component value has trailing bytes"));
        }
        Ok(value)
    }
}

fn int(value: &Nbt) -> Result<i32> {
    match value {
        Nbt::Byte(v) => Ok(i32::from(*v)),
        Nbt::Short(v) => Ok(i32::from(*v)),
        Nbt::Int(v) => Ok(*v),
        _ => Err(Error::Invalid("expected a number")),
    }
}

fn entries(value: &Nbt) -> Result<&[(String, Nbt)]> {
    match value {
        Nbt::Compound(entries) => Ok(entries),
        _ => Err(Error::Invalid("expected a compound")),
    }
}

impl Shape {
    fn resolve(&self) -> &Shape {
        match self {
            Shape::Lazy(f) => f().resolve(),
            other => other,
        }
    }

    /// Reads a value from the wire into its saved form.
    pub fn read(&self, buf: &mut &[u8], registries: &dyn Registries) -> Result<Nbt> {
        Ok(match self.resolve() {
            Shape::VarInt => Nbt::Int(VarInt::decode(buf)?.0),
            Shape::Int => Nbt::Int(i32::decode(buf)?),
            Shape::Float => Nbt::Float(f32::decode(buf)?),
            Shape::Bool => Nbt::Byte(i8::from(bool::decode(buf)?)),
            Shape::String => Nbt::String(decode_string(buf, 32767)?),
            Shape::Tag => Nbt::decode(buf)?,
            Shape::Unit => Nbt::Compound(Vec::new()),
            Shape::Holder(registry) => {
                let id = VarInt::decode(buf)?.0;
                Nbt::String(
                    registries
                        .name(registry, id)
                        .ok_or(Error::Invalid("unknown registry entry"))?,
                )
            }
            Shape::Enum(names) => {
                let id = VarInt::decode(buf)?.0;
                // `ByIdMap.OutOfBoundsStrategy.ZERO`.
                let name = usize::try_from(id)
                    .ok()
                    .and_then(|i| names.get(i))
                    .unwrap_or(&names[0]);
                Nbt::String((*name).into())
            }
            Shape::List(element) => {
                let count = decode_count(buf, buf.len())?;
                Nbt::List(
                    (0..count)
                        .map(|_| element.read(buf, registries))
                        .collect::<Result<_>>()?,
                )
            }
            Shape::Map(key, value) => {
                let count = decode_count(buf, buf.len())?;
                let mut entries = Vec::with_capacity(count);
                for _ in 0..count {
                    let Nbt::String(name) = key.read(buf, registries)? else {
                        return Err(Error::Invalid("map keys must be names"));
                    };
                    let value = value.read(buf, registries)?;
                    entries.retain(|(k, _)| *k != name);
                    entries.push((name, value));
                }
                Nbt::Compound(entries)
            }
            Shape::Record(fields) => {
                let mut entries = Vec::new();
                for field in *fields {
                    let present = match field.presence {
                        Presence::Optional => bool::decode(buf)?,
                        _ => true,
                    };
                    if !present {
                        continue;
                    }
                    let value = field.shape.read(buf, registries)?;
                    if matches!(&field.presence, Presence::Default(d) if *d == value) {
                        continue;
                    }
                    entries.push((field.name.to_owned(), value));
                }
                Nbt::Compound(entries)
            }
            Shape::Lazy(_) => unreachable!("resolved"),
        })
    }

    /// Reads past a value on the wire without looking up registry entries.
    pub fn skip(&self, buf: &mut &[u8]) -> Result<()> {
        match self.resolve() {
            Shape::VarInt | Shape::Holder(_) | Shape::Enum(_) => {
                VarInt::decode(buf)?;
            }
            Shape::Int => {
                i32::decode(buf)?;
            }
            Shape::Float => {
                f32::decode(buf)?;
            }
            Shape::Bool => {
                bool::decode(buf)?;
            }
            Shape::String => {
                decode_string(buf, 32767)?;
            }
            Shape::Tag => {
                Nbt::decode(buf)?;
            }
            Shape::Unit => {}
            Shape::List(element) => {
                for _ in 0..decode_count(buf, buf.len())? {
                    element.skip(buf)?;
                }
            }
            Shape::Map(key, value) => {
                for _ in 0..decode_count(buf, buf.len())? {
                    key.skip(buf)?;
                    value.skip(buf)?;
                }
            }
            Shape::Record(fields) => {
                for field in *fields {
                    let present = match field.presence {
                        Presence::Optional => bool::decode(buf)?,
                        _ => true,
                    };
                    if present {
                        field.shape.skip(buf)?;
                    }
                }
            }
            Shape::Lazy(_) => unreachable!("resolved"),
        }
        Ok(())
    }

    /// Writes a value in its saved form to the wire; fails if it does not
    /// fit the shape.
    pub fn write(&self, value: &Nbt, out: &mut Vec<u8>, registries: &dyn Registries) -> Result<()> {
        match self.resolve() {
            Shape::VarInt => VarInt(int(value)?).encode(out),
            Shape::Int => int(value)?.encode(out),
            Shape::Float => match value {
                Nbt::Float(v) => v.encode(out),
                Nbt::Double(v) => (*v as f32).encode(out),
                other => (int(other)? as f32).encode(out),
            },
            Shape::Bool => (int(value)? != 0).encode(out),
            Shape::String => match value {
                Nbt::String(s) => s.encode(out),
                _ => return Err(Error::Invalid("expected a string")),
            },
            Shape::Tag => value.encode(out),
            Shape::Unit => {}
            Shape::Holder(registry) => {
                let Nbt::String(name) = value else {
                    return Err(Error::Invalid("expected a registry name"));
                };
                let id = registries
                    .id(registry, &qualified(name))
                    .ok_or(Error::Invalid("unknown registry entry"))?;
                VarInt(id).encode(out);
            }
            Shape::Enum(names) => {
                let Nbt::String(name) = value else {
                    return Err(Error::Invalid("expected a name"));
                };
                let id = names
                    .iter()
                    .position(|n| n == name)
                    .ok_or(Error::Invalid("unknown enum value"))?;
                VarInt(id as i32).encode(out);
            }
            Shape::List(element) => {
                let Nbt::List(elements) = value else {
                    return Err(Error::Invalid("expected a list"));
                };
                VarInt(elements.len() as i32).encode(out);
                for e in elements {
                    element.write(e, out, registries)?;
                }
            }
            Shape::Map(key, element) => {
                let entries = entries(value)?;
                VarInt(entries.len() as i32).encode(out);
                for (name, v) in entries {
                    key.write(&Nbt::String(name.clone()), out, registries)?;
                    element.write(v, out, registries)?;
                }
            }
            Shape::Record(fields) => {
                let entries = entries(value)?;
                for field in *fields {
                    let given = entries.iter().find(|(k, _)| k == field.name).map(|e| &e.1);
                    match (&field.presence, given) {
                        (Presence::Optional, None) => false.encode(out),
                        (Presence::Optional, Some(v)) => {
                            true.encode(out);
                            field.shape.write(v, out, registries)?;
                        }
                        (Presence::Default(d) | Presence::Fallback(d), None) => {
                            field.shape.write(d, out, registries)?;
                        }
                        (_, Some(v)) => field.shape.write(v, out, registries)?,
                        (Presence::Required, None) => {
                            return Err(Error::Invalid("a required field is missing"));
                        }
                    }
                }
            }
            Shape::Lazy(_) => unreachable!("resolved"),
        }
        Ok(())
    }
}

/// `minecraft:` in front of a name without a namespace, as
/// `Identifier.parse` reads it.
fn qualified(name: &str) -> String {
    if name.contains(':') {
        name.to_owned()
    } else {
        format!("minecraft:{name}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixed;

    impl Registries for Fixed {
        fn name(&self, registry: &str, id: i32) -> Option<String> {
            let names: &[&str] = match registry {
                "minecraft:enchantment" => &["minecraft:aqua_affinity", "minecraft:sharpness"],
                "minecraft:mob_effect" => &["minecraft:speed", "minecraft:slowness"],
                "minecraft:potion" => &["minecraft:water", "minecraft:swiftness"],
                _ => return None,
            };
            names
                .get(usize::try_from(id).ok()?)
                .map(|n| (*n).to_owned())
        }

        fn id(&self, registry: &str, name: &str) -> Option<i32> {
            (0..8).find(|&id| self.name(registry, id).as_deref() == Some(name))
        }
    }

    fn round_trip(name: &str, saved: Nbt, wire: &[u8]) {
        let component = component(name).unwrap();
        assert_eq!(component.to_wire(&saved, &Fixed).unwrap(), wire, "{name}");
        assert_eq!(component.from_wire(wire, &Fixed).unwrap(), saved, "{name}");
    }

    #[test]
    fn components_on_the_wire_and_saved() {
        round_trip("minecraft:damage", Nbt::Int(300), &[0xac, 0x02]);
        round_trip("minecraft:unbreakable", Nbt::Compound(vec![]), &[]);
        round_trip(
            "minecraft:enchantments",
            Nbt::Compound(vec![("minecraft:sharpness".into(), Nbt::Int(5))]),
            &[1, 1, 5],
        );
        round_trip("minecraft:rarity", Nbt::String("epic".into()), &[3]);
        round_trip(
            "minecraft:enchantable",
            Nbt::Compound(vec![("value".into(), Nbt::Int(10))]),
            &[10],
        );
        // A potion: present, no colour, one custom effect, no name.
        round_trip(
            "minecraft:potion_contents",
            Nbt::Compound(vec![
                ("potion".into(), Nbt::String("minecraft:swiftness".into())),
                (
                    "custom_effects".into(),
                    Nbt::List(vec![Nbt::Compound(vec![
                        ("id".into(), Nbt::String("minecraft:slowness".into())),
                        ("amplifier".into(), Nbt::Int(1)),
                        ("duration".into(), Nbt::Int(200)),
                        ("show_icon".into(), Nbt::Byte(1)),
                    ])]),
                ),
            ]),
            &[1, 1, 0, 1, 1, 1, 0xc8, 0x01, 0, 1, 1, 0, 0],
        );
    }

    #[test]
    fn alternatives_and_defaults() {
        let potion = component("minecraft:potion_contents").unwrap();
        // Saved as only the name, as vanilla also accepts.
        let wire = potion
            .to_wire(&Nbt::String("swiftness".into()), &Fixed)
            .unwrap();
        assert_eq!(wire, [1, 1, 0, 0, 0]);
        assert_eq!(
            potion.from_wire(&wire, &Fixed).unwrap(),
            Nbt::Compound(vec![(
                "potion".into(),
                Nbt::String("minecraft:swiftness".into())
            )])
        );
        // Unknown entries and wrong types are refused.
        let enchantments = component("minecraft:enchantments").unwrap();
        assert!(
            enchantments
                .to_wire(
                    &Nbt::Compound(vec![("minecraft:nothing".into(), Nbt::Int(1))]),
                    &Fixed
                )
                .is_err()
        );
        assert!(
            component("minecraft:damage")
                .unwrap()
                .to_wire(&Nbt::String("x".into()), &Fixed)
                .is_err()
        );
        assert!(
            component("minecraft:damage")
                .unwrap()
                .from_wire(&[1, 2], &Fixed)
                .is_err()
        );
    }
}
