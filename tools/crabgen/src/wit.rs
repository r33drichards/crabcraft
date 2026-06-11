//! WIT → IR. Loads a .wit file with wit-parser (the same parser wasm-tools
//! uses, so `schema_json` matches `wasm-tools component wit --json`).

use std::path::Path;

use anyhow::{bail, Context, Result};
use wit_parser::{
    FunctionKind, Interface, InterfaceId, Resolve, Type, TypeDefKind, TypeOwner, WorldItem,
};

use crate::ir::{Func, Iface, Module, NamedTy, Ty};

/// Load a WIT file into the crabgen IR. v1 supports a single world with
/// exactly one exported interface and any number of imported interfaces.
pub fn load(path: &Path) -> Result<Module> {
    let mut resolve = Resolve::default();
    let (pkg_id, _) = resolve
        .push_path(path)
        .with_context(|| format!("parsing WIT at {}", path.display()))?;

    let pkg = &resolve.packages[pkg_id];
    let package = pkg.name.to_string();

    let mut worlds = pkg.worlds.values();
    let world_id = match (worlds.next(), worlds.next()) {
        (Some(&w), None) => w,
        (None, _) => {
            bail!("package `{package}` defines no world; add `world <name> {{ export <iface>; }}`")
        }
        (Some(_), Some(_)) => bail!(
            "package `{package}` defines more than one world; crabgen v1 supports exactly one"
        ),
    };
    let world = &resolve.worlds[world_id];

    let mut exports = Vec::new();
    for (_, item) in &world.exports {
        match item {
            WorldItem::Interface { id, .. } => exports.push(
                lower_iface(&resolve, *id)
                    .with_context(|| format!("in exports of world `{}`", world.name))?,
            ),
            WorldItem::Function(f) => bail!(
                "world `{}` exports function `{}` at the top level; unsupported in v1 — move it into an exported interface",
                world.name, f.name
            ),
            WorldItem::Type { .. } => bail!(
                "world `{}` exports a bare type; unsupported in v1 — declare types inside the exported interface",
                world.name
            ),
        }
    }
    match exports.len() {
        0 => bail!(
            "world `{}` exports no interface; crabgen v1 requires exactly one exported interface",
            world.name
        ),
        1 => {}
        n => bail!(
            "world `{}` exports {n} interfaces; crabgen v1 supports exactly one exported interface",
            world.name
        ),
    }

    let mut imports = Vec::new();
    for (_, item) in &world.imports {
        match item {
            WorldItem::Interface { id, .. } => imports.push(
                lower_iface(&resolve, *id)
                    .with_context(|| format!("in imports of world `{}`", world.name))?,
            ),
            WorldItem::Function(f) => bail!(
                "world `{}` imports function `{}` at the top level; unsupported in v1 — move it into an imported interface",
                world.name, f.name
            ),
            // world-level `use` shows up as type imports; nothing to generate
            WorldItem::Type { .. } => {}
        }
    }

    let schema_json =
        serde_json::to_string(&resolve).context("serializing resolved WIT to JSON")?;

    Ok(Module {
        package,
        world: world.name.clone(),
        exports,
        imports,
        schema_json,
    })
}

fn lower_iface(resolve: &Resolve, id: InterfaceId) -> Result<Iface> {
    let iface: &Interface = &resolve.interfaces[id];
    let instance = resolve
        .id_of(id)
        .context("interface has no package-qualified id")?;

    let mut types = Vec::new();
    for (name, &tid) in &iface.types {
        let td = &resolve.types[tid];
        // `use other.{t};` resolves to an alias whose target lives in another
        // interface; following it would emit a self-referential Named and drag
        // the type-provider interface into Module.imports as a mesh stub.
        if let TypeDefKind::Type(Type::Id(orig)) = &td.kind {
            if resolve.types[*orig].owner != TypeOwner::Interface(id) {
                bail!(
                    "`use` across interfaces is unsupported in v1 — declare the type in the interface that uses it (type `{name}` in `{instance}`)"
                );
            }
        }
        let ty = lower_typedef_kind(resolve, &td.kind)
            .with_context(|| format!("in type `{name}` of `{instance}`"))?;
        types.push(NamedTy {
            wit_name: name.clone(),
            ty,
        });
    }

    let mut funcs = Vec::new();
    for (name, f) in &iface.functions {
        if !matches!(f.kind, FunctionKind::Freestanding) {
            bail!("unsupported function kind for `{name}` in `{instance}`: only plain (freestanding, non-async) functions work in v1");
        }
        let params = f
            .params
            .iter()
            .map(|p| Ok((p.name.clone(), lower_type(resolve, &p.ty)?)))
            .collect::<Result<Vec<_>>>()
            .with_context(|| format!("in params of `{name}` in `{instance}`"))?;
        let result = f
            .result
            .map(|rty| lower_type(resolve, &rty))
            .transpose()
            .with_context(|| format!("in result of `{name}` in `{instance}`"))?;
        funcs.push(Func {
            wit_name: name.clone(),
            params,
            result,
        });
    }

    Ok(Iface {
        instance,
        funcs,
        types,
    })
}

fn lower_type(resolve: &Resolve, ty: &Type) -> Result<Ty> {
    Ok(match ty {
        Type::Bool => Ty::Bool,
        Type::U8 => Ty::U8,
        Type::U16 => Ty::U16,
        Type::U32 => Ty::U32,
        Type::U64 => Ty::U64,
        Type::S8 => Ty::S8,
        Type::S16 => Ty::S16,
        Type::S32 => Ty::S32,
        Type::S64 => Ty::S64,
        Type::F32 => Ty::F32,
        Type::F64 => Ty::F64,
        Type::Char => Ty::Char,
        Type::String => Ty::String,
        Type::Id(tid) => {
            let td = &resolve.types[*tid];
            // a named type is emitted once in Iface::types and referenced here
            match &td.name {
                Some(name) => Ty::Named(name.clone()),
                None => lower_typedef_kind(resolve, &td.kind)?,
            }
        }
        other => bail!("unsupported WIT type {other:?}: not in the WIRE v0 value encoding"),
    })
}

fn lower_typedef_kind(resolve: &Resolve, kind: &TypeDefKind) -> Result<Ty> {
    Ok(match kind {
        TypeDefKind::Record(r) => Ty::Record(
            r.fields
                .iter()
                .map(|f| Ok((f.name.clone(), lower_type(resolve, &f.ty)?)))
                .collect::<Result<Vec<_>>>()?,
        ),
        TypeDefKind::Tuple(t) => Ty::Tuple(
            t.types
                .iter()
                .map(|ty| lower_type(resolve, ty))
                .collect::<Result<Vec<_>>>()?,
        ),
        TypeDefKind::Variant(v) => Ty::Variant(
            v.cases
                .iter()
                .map(|c| {
                    Ok((
                        c.name.clone(),
                        c.ty.as_ref()
                            .map(|ty| lower_type(resolve, ty))
                            .transpose()?,
                    ))
                })
                .collect::<Result<Vec<_>>>()?,
        ),
        TypeDefKind::Enum(e) => Ty::Enum(e.cases.iter().map(|c| c.name.clone()).collect()),
        TypeDefKind::Flags(f) => Ty::Flags(f.flags.iter().map(|f| f.name.clone()).collect()),
        TypeDefKind::Option(t) => Ty::Option(Box::new(lower_type(resolve, t)?)),
        TypeDefKind::List(t) => Ty::List(Box::new(lower_type(resolve, t)?)),
        TypeDefKind::Result(r) => {
            let ok = match &r.ok {
                Some(t) => Some(Box::new(lower_type(resolve, t)?)),
                None => None,
            };
            let err = match &r.err {
                Some(t) => Some(Box::new(lower_type(resolve, t)?)),
                None => None,
            };
            Ty::Result(ok, err)
        }
        TypeDefKind::Type(t) => lower_type(resolve, t)?,
        TypeDefKind::Resource | TypeDefKind::Handle(_) => {
            bail!("unsupported: resources are not part of the crab ABI (WIRE v0); model state as records + functions instead")
        }
        TypeDefKind::Future(_) | TypeDefKind::Stream(_) => {
            bail!("unsupported: streams/futures are not implemented in WIRE v0")
        }
        other => bail!("unsupported WIT construct {other:?}: not in the WIRE v0 value encoding"),
    })
}
