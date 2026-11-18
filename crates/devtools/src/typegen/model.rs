//! The package description that `crates/devtools/go/typegen` writes (go/types facts about
//! each Go package, in source order).

use serde::Deserialize;

/// The describer's whole output.
#[derive(Debug, Clone, Deserialize)]
pub struct Description {
    /// The described packages, in the order requested.
    pub packages: Vec<Package>,
}

/// One Go package.
#[derive(Debug, Clone, Deserialize)]
pub struct Package {
    /// The import path.
    pub path: String,
    /// The package name (what `reflect.Type.String()` prints before the dot).
    pub name: String,
    /// The module path.
    #[serde(default)]
    pub module: String,
    /// The module version (empty for the main module).
    #[serde(default)]
    pub version: String,
    /// The package's `.go` files, sorted.
    #[serde(default)]
    pub files: Vec<String>,
    /// Every top-level declaration, in file and source order.
    #[serde(default)]
    pub decls: Vec<Decl>,
}

/// What a declaration declares.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DeclKind {
    /// `type T struct { ... }`
    Struct,
    /// `type T U` for a non-struct, non-interface `U`.
    Named,
    /// `type T = U`
    Alias,
    /// `type T interface { ... }`
    Interface,
    /// A constant.
    Const,
    /// A function.
    Func,
    /// A method.
    Method,
}

/// One top-level declaration.
#[derive(Debug, Clone, Deserialize)]
pub struct Decl {
    /// What it declares.
    pub kind: DeclKind,
    /// The Go name.
    pub name: String,
    /// The file (base name) it is declared in.
    pub file: String,
    /// Whether the name is exported.
    pub exported: bool,
    /// The doc comment (`ast.CommentGroup.Text()`).
    #[serde(default)]
    pub doc: String,
    /// The trailing line comment.
    #[serde(default)]
    pub comment: String,
    /// A generic type or function.
    #[serde(default)]
    pub generic: bool,
    /// A struct's fields.
    #[serde(default)]
    pub fields: Vec<Field>,
    /// A named type's underlying type, an alias's target or a constant's type.
    #[serde(default, rename = "type")]
    pub ty: Option<TypeRef>,
    /// The JSON-related methods of a named type.
    #[serde(default)]
    pub json_methods: Option<JsonMethods>,
    /// A constant's exact value.
    #[serde(default)]
    pub value: String,
    /// `int`, `float`, `string`, `bool` or `other`.
    #[serde(default)]
    pub value_kind: String,
    /// The const block a constant belongs to (1-based within the package).
    #[serde(default)]
    pub group: u32,
    /// The doc comment of that const block.
    #[serde(default)]
    pub group_doc: String,
    /// A method's receiver type name.
    #[serde(default)]
    pub recv: String,
    /// Whether a method has a pointer receiver.
    #[serde(default)]
    pub ptr_recv: bool,
    /// A function's or method's Go signature (`func(v int8) *int8`).
    #[serde(default)]
    pub signature: String,
}

/// One struct field.
#[derive(Debug, Clone, Deserialize)]
pub struct Field {
    /// The Go field name (the type name for an embedded field).
    pub name: String,
    /// The field type.
    #[serde(rename = "type")]
    pub ty: TypeRef,
    /// The whole struct tag.
    #[serde(default)]
    pub tag: String,
    /// The tag's `json` key, when present.
    #[serde(default)]
    pub json: Option<String>,
    /// An embedded field.
    #[serde(default)]
    pub embedded: bool,
    /// Whether the field is exported.
    #[serde(default)]
    pub exported: bool,
    /// The doc comment.
    #[serde(default)]
    pub doc: String,
    /// The trailing line comment.
    #[serde(default)]
    pub comment: String,
}

/// The shape of a type expression.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TypeKind {
    /// A predeclared type (`int8`, `string`, `untyped int`, ...).
    Basic,
    /// A defined type.
    Named,
    /// `*T`
    Pointer,
    /// `[]T`
    Slice,
    /// `[N]T`
    Array,
    /// `map[K]V`
    Map,
    /// An interface type.
    Interface,
    /// An unnamed struct type.
    Struct,
    /// A function type.
    Func,
    /// A channel type.
    Chan,
    /// A type parameter.
    Typeparam,
}

/// A type expression.
#[derive(Debug, Clone, Deserialize)]
pub struct TypeRef {
    /// The shape.
    pub kind: TypeKind,
    /// A basic type's name, or a named type's name.
    #[serde(default)]
    pub name: String,
    /// A named type's import path.
    #[serde(default)]
    pub pkg: String,
    /// A named type's package name.
    #[serde(default)]
    pub pkg_name: String,
    /// A generic instantiation's type arguments.
    #[serde(default)]
    pub args: Vec<Self>,
    /// The element type of a pointer, slice, array, map or channel.
    #[serde(default)]
    pub elem: Option<Box<Self>>,
    /// A map's key type.
    #[serde(default)]
    pub key: Option<Box<Self>>,
    /// An unnamed struct's fields.
    #[serde(default)]
    pub fields: Vec<Field>,
    /// An interface's method names.
    #[serde(default)]
    pub methods: Vec<String>,
    /// `reflect.Type.String()`.
    pub reflect: String,
}

/// Which JSON-related methods a named type has: `value` (on `T`) or `pointer` (only on `*T`).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct JsonMethods {
    /// `MarshalJSON`
    #[serde(default)]
    pub marshal_json: String,
    /// `UnmarshalJSON`
    #[serde(default)]
    pub unmarshal_json: String,
    /// `MarshalText`
    #[serde(default)]
    pub marshal_text: String,
    /// `UnmarshalText`
    #[serde(default)]
    pub unmarshal_text: String,
}
