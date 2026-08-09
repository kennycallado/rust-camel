use proc_macro2::TokenStream;
use quote::quote;
use syn::{
    Data, DeriveInput, Fields, Lit, Meta, Token, Type, TypePath, parse::Parse, parse::ParseStream,
};

// ---------------------------------------------------------------------------
// UriParamAttr — parsed `#[uri_param]` attribute
// ---------------------------------------------------------------------------

/// Parsed `#[uri_param]` attribute.
///
/// Supports both bare-ident flags (`required`, `secret`) and `key = value`
/// pairs. See the attribute table in `lib.rs` for the full key set.
#[derive(Clone, Debug, Default)]
struct UriParamAttr {
    /// Custom parameter name (`name = "..."`).
    name: Option<String>,
    /// Default value (`default = "..."`).
    default: Option<String>,
    /// Human-readable description (`desc = "..."`).
    desc: Option<String>,
    /// `required` flag (bare or `required = bool`).
    required: bool,
    /// `secret` flag (bare or `secret = bool`).
    secret: bool,
    /// Deprecation reason (`deprecated = "..."`).
    deprecated: Option<String>,
    /// Alias names (`aliases = ["a", "b"]`).
    aliases: Vec<String>,
    /// OptionKind override (`kind = "duration"` / `kind = "enum:A,B"`), with
    /// the literal's span preserved for spanned error reporting.
    kind_override: Option<syn::LitStr>,
}

impl Parse for UriParamAttr {
    /// Parse a comma-separated list of EITHER bare-ident flags
    /// (`required`, `secret`) OR `key = value` pairs.
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let mut attr = UriParamAttr::default();

        while !input.is_empty() {
            let ident: syn::Ident = input.parse()?;
            let key_str = ident.to_string();

            if input.peek(Token![=]) {
                input.parse::<Token![=]>()?;
                match key_str.as_str() {
                    "name" | "default" | "desc" | "deprecated" => {
                        let lit: Lit = input.parse()?;
                        if let Lit::Str(lit_str) = lit {
                            let val = lit_str.value();
                            match key_str.as_str() {
                                "name" => attr.name = Some(val),
                                "default" => attr.default = Some(val),
                                "desc" => attr.desc = Some(val),
                                "deprecated" => attr.deprecated = Some(val),
                                _ => unreachable!(),
                            }
                        } else {
                            return Err(syn::Error::new_spanned(lit, "expected a string literal"));
                        }
                    }
                    "kind" => {
                        let lit: Lit = input.parse()?;
                        if let Lit::Str(lit_str) = lit {
                            attr.kind_override = Some(lit_str);
                        } else {
                            return Err(syn::Error::new_spanned(lit, "expected a string literal"));
                        }
                    }
                    "required" | "secret" => {
                        let lit: Lit = input.parse()?;
                        if let Lit::Bool(b) = lit {
                            let v = b.value;
                            if key_str == "required" {
                                attr.required = v;
                            } else {
                                attr.secret = v;
                            }
                        } else {
                            return Err(syn::Error::new_spanned(lit, "expected a bool literal"));
                        }
                    }
                    "aliases" => {
                        let arr: syn::ExprArray = input.parse()?;
                        let mut items = Vec::new();
                        for expr in arr.elems {
                            if let syn::Expr::Lit(syn::ExprLit {
                                lit: Lit::Str(s), ..
                            }) = expr
                            {
                                items.push(s.value());
                            } else {
                                return Err(syn::Error::new_spanned(
                                    expr,
                                    "expected a string literal in aliases array",
                                ));
                            }
                        }
                        attr.aliases = items;
                    }
                    _ => {
                        return Err(syn::Error::new_spanned(
                            &ident,
                            format!("unknown attribute key: {}", key_str),
                        ));
                    }
                }
            } else {
                // Bare-ident flag form.
                match key_str.as_str() {
                    "required" => attr.required = true,
                    "secret" => attr.secret = true,
                    _ => {
                        return Err(syn::Error::new_spanned(
                            &ident,
                            format!("unknown attribute key: {}", key_str),
                        ));
                    }
                }
            }

            // Optional comma separator.
            if input.peek(Token![,]) {
                input.parse::<Token![,]>()?;
            }
        }

        Ok(attr)
    }
}

/// Extract the URI scheme from struct attributes (`#[uri_scheme = "xxx"]`).
fn extract_scheme(attrs: &[syn::Attribute]) -> syn::Result<String> {
    for attr in attrs {
        if let Meta::NameValue(nv) = &attr.meta
            && nv.path.is_ident("uri_scheme")
            && let syn::Expr::Lit(expr_lit) = &nv.value
            && let Lit::Str(lit_str) = &expr_lit.lit
        {
            return Ok(lit_str.value());
        }
    }
    Err(syn::Error::new(
        proc_macro2::Span::call_site(),
        "missing #[uri_scheme = \"xxx\"] attribute on struct",
    ))
}

/// Parse a `#[uri_param]` attribute from field attributes.
///
/// Returns `Ok(Some(attr))` when `#[uri_param]` is present (bare or with
/// args), `Ok(None)` when absent.
fn parse_uri_param_attr(attrs: &[syn::Attribute]) -> syn::Result<Option<UriParamAttr>> {
    for attr in attrs {
        if attr.path().is_ident("uri_param") {
            match &attr.meta {
                Meta::Path(_) => {
                    // Bare `#[uri_param]` — all flags/options default.
                    return Ok(Some(UriParamAttr::default()));
                }
                Meta::List(list) => {
                    let parsed: UriParamAttr = list.parse_args()?;
                    return Ok(Some(parsed));
                }
                _ => {
                    return Err(syn::Error::new_spanned(
                        attr,
                        "unexpected attribute format for #[uri_param]",
                    ));
                }
            }
        }
    }
    Ok(None)
}

// ---------------------------------------------------------------------------
// UriConfigAttr — parsed `#[uri_config(...)]` struct attribute
// ---------------------------------------------------------------------------

struct UriConfigAttr {
    skip_impl: bool,
    crate_path: syn::Path,
    has_metadata: bool,
    metadata_scheme: Option<String>,
    metadata_description: Option<String>,
    supports_producer: bool,
    supports_consumer: bool,
    supports_polling_consumer: bool,
    supports_streaming: bool,
}

fn parse_uri_config_attr(attrs: &[syn::Attribute]) -> syn::Result<UriConfigAttr> {
    let mut skip_impl = false;
    let mut crate_path: Option<syn::Path> = None;
    let mut has_metadata = false;
    let mut metadata_scheme = None;
    let mut metadata_description = None;
    let mut supports_producer = false;
    let mut supports_consumer = false;
    let mut supports_polling_consumer = false;
    let mut supports_streaming = false;

    for attr in attrs {
        if !attr.path().is_ident("uri_config") {
            continue;
        }

        match &attr.meta {
            Meta::List(_) => {
                attr.parse_nested_meta(|meta| {
                    if meta.path.is_ident("skip_impl") {
                        skip_impl = true;
                        return Ok(());
                    }

                    if meta.path.is_ident("crate") {
                        let value = meta.value()?;
                        let lit: syn::LitStr = value.parse()?;
                        crate_path = Some(lit.parse()?);
                        return Ok(());
                    }

                    if meta.path.is_ident("metadata") {
                        // C-NEW-2: `metadata(..)` mixes bare flags (`producer`)
                        // with kv pairs (`scheme = ".."`). Parse the
                        // parenthesized group, then walk it manually so both
                        // forms are accepted.
                        has_metadata = true;
                        let content;
                        syn::parenthesized!(content in meta.input);
                        while !content.is_empty() {
                            let key: syn::Ident = content.parse()?;
                            match key.to_string().as_str() {
                                "scheme" => {
                                    content.parse::<Token![=]>()?;
                                    let lit: syn::LitStr = content.parse()?;
                                    metadata_scheme = Some(lit.value());
                                }
                                "description" => {
                                    content.parse::<Token![=]>()?;
                                    let lit: syn::LitStr = content.parse()?;
                                    metadata_description = Some(lit.value());
                                }
                                "producer" => supports_producer = true,
                                "consumer" => supports_consumer = true,
                                "polling_consumer" => supports_polling_consumer = true,
                                "streaming" => supports_streaming = true,
                                other => {
                                    return Err(syn::Error::new_spanned(
                                        &key,
                                        format!("unknown metadata key: {}", other),
                                    ));
                                }
                            }
                            if content.peek(Token![,]) {
                                content.parse::<Token![,]>()?;
                            }
                        }
                        return Ok(());
                    }

                    Err(meta.error("unknown uri_config option"))
                })?;
            }
            _ => {
                return Err(syn::Error::new_spanned(
                    attr,
                    "unexpected attribute format for #[uri_config]",
                ));
            }
        }
    }

    Ok(UriConfigAttr {
        skip_impl,
        crate_path: crate_path.unwrap_or_else(|| syn::parse_quote!(camel_endpoint)),
        has_metadata,
        metadata_scheme,
        metadata_description,
        supports_producer,
        supports_consumer,
        supports_polling_consumer,
        supports_streaming,
    })
}

// ---------------------------------------------------------------------------
// Type inspection helpers
// ---------------------------------------------------------------------------

/// Get the type name as a string (for simple type matching).
fn get_type_name(ty: &Type) -> Option<String> {
    if let Type::Path(TypePath { path, .. }) = ty {
        let segment = path.segments.last()?;
        Some(segment.ident.to_string())
    } else {
        None
    }
}

/// Check if a type is `std::time::Duration`.
fn is_duration_type(ty: &Type) -> bool {
    if let Type::Path(TypePath { path, .. }) = ty {
        let segments: Vec<_> = path.segments.iter().map(|s| s.ident.to_string()).collect();
        segments.last().is_some_and(|s| s == "Duration")
    } else {
        false
    }
}

/// Unwrap `Option<T>` to its inner type, if applicable.
fn is_option_type(ty: &Type) -> Option<Type> {
    if let Type::Path(TypePath { path, .. }) = ty {
        let segment = path.segments.last()?;
        if segment.ident == "Option"
            && let syn::PathArguments::AngleBracketed(args) = &segment.arguments
            && let Some(syn::GenericArgument::Type(inner_ty)) = args.args.first()
        {
            return Some(inner_ty.clone());
        }
    }
    None
}

/// Unwrap `Vec<T>` to its inner type, if applicable.
fn get_vec_inner(ty: &Type) -> Option<Type> {
    if let Type::Path(TypePath { path, .. }) = ty {
        let segment = path.segments.last()?;
        if segment.ident == "Vec"
            && let syn::PathArguments::AngleBracketed(args) = &segment.arguments
            && let Some(syn::GenericArgument::Type(inner_ty)) = args.args.first()
        {
            return Some(inner_ty.clone());
        }
    }
    None
}

// ---------------------------------------------------------------------------
// OptionKind inference (task 1.2)
// ---------------------------------------------------------------------------

/// Resolve a `kind = "..."` override string to an `OptionKind` constructor
/// token stream. Returns a spanned `syn::Error` for unrecognized strings.
///
/// Valid: `duration`, `bool`, `int`, `float`, `string`, `enum:A,B,C`.
fn parse_kind_override(
    kind_str: &str,
    span: proc_macro2::Span,
    endpoint_crate: &syn::Path,
) -> syn::Result<TokenStream> {
    let path = quote! { #endpoint_crate::OptionKind };
    match kind_str {
        "duration" => Ok(quote! { #path::Duration }),
        "bool" => Ok(quote! { #path::Bool }),
        "int" => Ok(quote! { #path::Int }),
        "float" => Ok(quote! { #path::Float }),
        "string" => Ok(quote! { #path::String }),
        s if s.starts_with("enum:") => {
            let rest = &s[5..];
            let variants: Vec<String> = rest
                .split(',')
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
                .collect();
            if variants.is_empty() {
                return Err(syn::Error::new(
                    span,
                    format!(
                        "invalid kind override '{}': enum requires at least one variant \
                         (e.g. kind = \"enum:A,B\")",
                        kind_str
                    ),
                ));
            }
            Ok(quote! { #path::Enum(::std::vec![#(::std::string::String::from(#variants)),*]) })
        }
        other => Err(syn::Error::new(
            span,
            // allow-secret: error message names kind overrides; lint window reaches the unrelated "token stream" doc
            format!(
                "unknown kind override '{}'. Valid: duration, bool, int, float, string, \
                 enum:VariantA,VariantB",
                other
            ),
        )),
    }
}

/// Map a Rust type to an `OptionKind` constructor token stream.
///
/// `Option<T>` is unwrapped to `T` first. **Inference NEVER emits `Enum`** —
/// an enum-typed field maps to `String`. Use `kind = "enum:..."` to opt into
/// `Enum`.
fn infer_option_kind(ty: &Type, endpoint_crate: &syn::Path) -> TokenStream {
    let effective_ty = is_option_type(ty).unwrap_or_else(|| ty.clone());
    infer_option_kind_inner(&effective_ty, endpoint_crate)
}

fn infer_option_kind_inner(ty: &Type, endpoint_crate: &syn::Path) -> TokenStream {
    let path = quote! { #endpoint_crate::OptionKind };

    if is_duration_type(ty) {
        return quote! { #path::Duration };
    }

    let type_name = get_type_name(ty);
    match type_name.as_deref() {
        Some("bool") => quote! { #path::Bool },
        Some("u8") | Some("u16") | Some("u32") | Some("u64") | Some("usize") | Some("i8")
        | Some("i16") | Some("i32") | Some("i64") | Some("isize") => quote! { #path::Int },
        Some("f32") | Some("f64") => quote! { #path::Float },
        Some("String") | Some("str") => quote! { #path::String },
        Some("Vec") => {
            if let Some(inner) = get_vec_inner(ty) {
                let inner_kind = infer_option_kind_inner(&inner, endpoint_crate);
                quote! { #path::List(::std::boxed::Box::new(#inner_kind)) }
            } else {
                quote! { #path::String }
            }
        }
        // Anything else (enums, custom types) infers to String — never Enum.
        _ => quote! { #path::String },
    }
}

// ---------------------------------------------------------------------------
// URI param parsing codegen (unchanged from original)
// ---------------------------------------------------------------------------

/// Generate code to parse a value from params into a local variable.
///
/// EMAC-005: Error messages include the URI parameter name (`param_name`) for
/// traceability. When `#[uri_param(name = "...")]` is used, the custom name
/// appears in errors; otherwise the Rust field name is used as the param name.
fn generate_param_parsing(
    param_name: &str,
    field_name: &syn::Ident,
    ty: &Type,
    default: Option<&str>,
    endpoint_crate: &syn::Path,
) -> syn::Result<TokenStream> {
    let type_name = get_type_name(ty);
    let inner_type = is_option_type(ty);

    // Handle Option<T>
    if let Some(inner_ty) = &inner_type {
        let inner_type_name = get_type_name(inner_ty);

        return Ok(match inner_type_name.as_deref() {
            Some("String") => quote! {
                let #field_name = params.get(#param_name).cloned()
            },
            Some("bool") => quote! {
                let #field_name = if let Some(v) = params.get(#param_name) {
                    Some(#endpoint_crate::uri::parse_bool_param(v).map_err(|e| #endpoint_crate::CamelError::InvalidUri(
                        format!("invalid value for {}: {}", #param_name, e)
                    ))?)
                } else {
                    None
                }
            },
            Some("u64") | Some("u32") | Some("usize") | Some("i64") | Some("i32")
            | Some("isize") => quote! {
                let #field_name = if let Some(v) = params.get(#param_name) {
                    Some(v.parse::<#inner_ty>().map_err(|e| #endpoint_crate::CamelError::InvalidUri(
                        format!("invalid value for {}: {}", #param_name, e)
                    ))?)
                } else {
                    None
                }
            },
            _ => quote! {
                let #field_name = if let Some(v) = params.get(#param_name) {
                    Some(v.parse::<#inner_ty>().map_err(|e| #endpoint_crate::CamelError::InvalidUri(
                        format!("invalid value for {}: {}", #param_name, e)
                    ))?)
                } else {
                    None
                }
            },
        });
    }

    // Handle non-Option types
    Ok(match type_name.as_deref() {
        Some("String") => {
            if let Some(default_val) = default {
                quote! {
                    let #field_name = params.get(#param_name).cloned().unwrap_or_else(|| #default_val.to_string())
                }
            } else {
                quote! {
                    let #field_name = params.get(#param_name).cloned().ok_or_else(|| {
                        #endpoint_crate::CamelError::InvalidUri(
                            format!("missing required parameter: {}", #param_name)
                        )
                    })?
                }
            }
        }
        Some("bool") => {
            if let Some(default_val) = default {
                let default_bool =
                    matches!(default_val.to_lowercase().as_str(), "true" | "1" | "yes");
                quote! {
                    let #field_name = match params.get(#param_name) {
                        Some(v) => #endpoint_crate::uri::parse_bool_param(v).map_err(|e| #endpoint_crate::CamelError::InvalidUri(
                            format!("invalid value for {}: {}", #param_name, e)
                        ))?,
                        None => #default_bool,
                    }
                }
            } else {
                // Require the param instead of silent false default
                quote! {
                    let #field_name = #endpoint_crate::uri::parse_bool_param(
                        &params.get(#param_name).ok_or_else(|| #endpoint_crate::CamelError::InvalidUri(
                            format!("missing required parameter: {}", #param_name)
                        ))?
                    ).map_err(|e| #endpoint_crate::CamelError::InvalidUri(
                        format!("invalid value for {}: {}", #param_name, e)
                    ))?
                }
            }
        }
        Some("u64") => {
            if let Some(default_val) = default {
                let default_num: u64 = default_val.parse().map_err(|_| {
                    syn::Error::new(
                        proc_macro2::Span::call_site(),
                        format!(
                            "invalid default value for '{}': '{}' is not a valid u64",
                            param_name, default_val
                        ),
                    )
                })?;
                quote! {
                    let #field_name = match params.get(#param_name) {
                        Some(v) => v.parse::<u64>().map_err(|e| #endpoint_crate::CamelError::InvalidUri(
                            format!("invalid value for {}: {}", #param_name, e)
                        ))?,
                        None => #default_num,
                    }
                }
            } else {
                quote! {
                    let #field_name = params.get(#param_name)
                        .ok_or_else(|| #endpoint_crate::CamelError::InvalidUri(
                            format!("missing required parameter: {}", #param_name)
                        ))?
                        .parse::<u64>()
                        .map_err(|e| #endpoint_crate::CamelError::InvalidUri(
                            format!("invalid value for {}: {}", #param_name, e)
                        ))?
                }
            }
        }
        Some("u32") => {
            if let Some(default_val) = default {
                let default_num: u32 = default_val.parse().map_err(|_| {
                    syn::Error::new(
                        proc_macro2::Span::call_site(),
                        format!(
                            "invalid default value for '{}': '{}' is not a valid u32",
                            param_name, default_val
                        ),
                    )
                })?;
                quote! {
                    let #field_name = match params.get(#param_name) {
                        Some(v) => v.parse::<u32>().map_err(|e| #endpoint_crate::CamelError::InvalidUri(
                            format!("invalid value for {}: {}", #param_name, e)
                        ))?,
                        None => #default_num,
                    }
                }
            } else {
                quote! {
                    let #field_name = params.get(#param_name)
                        .ok_or_else(|| #endpoint_crate::CamelError::InvalidUri(
                            format!("missing required parameter: {}", #param_name)
                        ))?
                        .parse::<u32>()
                        .map_err(|e| #endpoint_crate::CamelError::InvalidUri(
                            format!("invalid value for {}: {}", #param_name, e)
                        ))?
                }
            }
        }
        Some("usize") => {
            if let Some(default_val) = default {
                let default_num: usize = default_val.parse().map_err(|_| {
                    syn::Error::new(
                        proc_macro2::Span::call_site(),
                        format!(
                            "invalid default value for '{}': '{}' is not a valid usize",
                            param_name, default_val
                        ),
                    )
                })?;
                quote! {
                    let #field_name = match params.get(#param_name) {
                        Some(v) => v.parse::<usize>().map_err(|e| #endpoint_crate::CamelError::InvalidUri(
                            format!("invalid value for {}: {}", #param_name, e)
                        ))?,
                        None => #default_num,
                    }
                }
            } else {
                quote! {
                    let #field_name = params.get(#param_name)
                        .ok_or_else(|| #endpoint_crate::CamelError::InvalidUri(
                            format!("missing required parameter: {}", #param_name)
                        ))?
                        .parse::<usize>()
                        .map_err(|e| #endpoint_crate::CamelError::InvalidUri(
                            format!("invalid value for {}: {}", #param_name, e)
                        ))?
                }
            }
        }
        Some("i64") => {
            if let Some(default_val) = default {
                let default_num: i64 = default_val.parse().map_err(|_| {
                    syn::Error::new(
                        proc_macro2::Span::call_site(),
                        format!(
                            "invalid default value for '{}': '{}' is not a valid i64",
                            param_name, default_val
                        ),
                    )
                })?;
                quote! {
                    let #field_name = match params.get(#param_name) {
                        Some(v) => v.parse::<i64>().map_err(|e| #endpoint_crate::CamelError::InvalidUri(
                            format!("invalid value for {}: {}", #param_name, e)
                        ))?,
                        None => #default_num,
                    }
                }
            } else {
                quote! {
                    let #field_name = params.get(#param_name)
                        .ok_or_else(|| #endpoint_crate::CamelError::InvalidUri(
                            format!("missing required parameter: {}", #param_name)
                        ))?
                        .parse::<i64>()
                        .map_err(|e| #endpoint_crate::CamelError::InvalidUri(
                            format!("invalid value for {}: {}", #param_name, e)
                        ))?
                }
            }
        }
        Some("i32") => {
            if let Some(default_val) = default {
                let default_num: i32 = default_val.parse().map_err(|_| {
                    syn::Error::new(
                        proc_macro2::Span::call_site(),
                        format!(
                            "invalid default value for '{}': '{}' is not a valid i32",
                            param_name, default_val
                        ),
                    )
                })?;
                quote! {
                    let #field_name = match params.get(#param_name) {
                        Some(v) => v.parse::<i32>().map_err(|e| #endpoint_crate::CamelError::InvalidUri(
                            format!("invalid value for {}: {}", #param_name, e)
                        ))?,
                        None => #default_num,
                    }
                }
            } else {
                quote! {
                    let #field_name = params.get(#param_name)
                        .ok_or_else(|| #endpoint_crate::CamelError::InvalidUri(
                            format!("missing required parameter: {}", #param_name)
                        ))?
                        .parse::<i32>()
                        .map_err(|e| #endpoint_crate::CamelError::InvalidUri(
                            format!("invalid value for {}: {}", #param_name, e)
                        ))?
                }
            }
        }
        _ => {
            // Assume it's an enum or other type with FromStr
            if let Some(default_val) = default {
                quote! {
                    let #field_name = match params.get(#param_name) {
                        Some(v) => v.parse::<#ty>().map_err(|e| #endpoint_crate::CamelError::InvalidUri(
                            format!("invalid value for parameter '{}': {}", #param_name, e)
                        ))?,
                        None => #default_val.parse::<#ty>().map_err(|e| #endpoint_crate::CamelError::InvalidUri(
                            format!("invalid default value for parameter '{}': {}", #param_name, e)
                        ))?,
                    };
                }
            } else {
                quote! {
                    let #field_name = params.get(#param_name)
                        .ok_or_else(|| #endpoint_crate::CamelError::InvalidUri(
                            format!("missing required parameter: {}", #param_name)
                        ))?
                        .parse::<#ty>()
                        .map_err(|e| #endpoint_crate::CamelError::InvalidUri(
                            format!("invalid value for parameter '{}': {}", #param_name, e)
                        ))?
                }
            }
        }
    })
}

// ---------------------------------------------------------------------------
// uri_options() generation (task 1.3)
// ---------------------------------------------------------------------------

/// Build the `UriOption` constructor expression (a builder-method chain) for
/// a single `#[uri_param]` field.
fn build_uri_option_entry(
    field_ident: &syn::Ident,
    field_type: &Type,
    attr: &UriParamAttr,
    endpoint_crate: &syn::Path,
) -> syn::Result<TokenStream> {
    // Guardrail: secret + default is a compile error.
    if attr.secret && attr.default.is_some() {
        return Err(syn::Error::new_spanned(
            field_ident,
            "#[uri_param] cannot have both `secret` and `default`; a secret \
             must never carry a default value",
        ));
    }

    let param_name = attr.name.clone().unwrap_or_else(|| field_ident.to_string());
    let description = attr.desc.clone().unwrap_or_default();

    // Resolve kind: explicit override wins; otherwise infer (never Enum).
    let kind_ts = if let Some(lit) = &attr.kind_override {
        parse_kind_override(&lit.value(), lit.span(), endpoint_crate)?
    } else {
        infer_option_kind(field_type, endpoint_crate)
    };

    // Required inference: explicit flag wins; else Option<T> => false;
    // else non-Option with a default => false; else => true.
    let is_option = is_option_type(field_type).is_some();
    let required = attr.required || (!is_option && attr.default.is_none());

    let mut chain = quote! {
        #endpoint_crate::UriOption::new(#param_name, #description, #kind_ts)
    };
    if required {
        chain = quote! { #chain.required() };
    }
    if let Some(default_val) = &attr.default {
        chain = quote! { #chain.with_default(#default_val) };
    }
    if attr.secret {
        chain = quote! { #chain.secret() };
    }
    if let Some(deprecated_reason) = &attr.deprecated {
        chain = quote! { #chain.deprecated(#deprecated_reason) };
    }
    for alias in &attr.aliases {
        chain = quote! { #chain.with_alias(#alias) };
    }

    Ok(chain)
}

// ---------------------------------------------------------------------------
// Main derive entrypoint
// ---------------------------------------------------------------------------

pub fn impl_uri_config(input: &DeriveInput) -> syn::Result<TokenStream> {
    let struct_name = &input.ident;

    let uri_config_attr = parse_uri_config_attr(&input.attrs)?;

    let skip_impl = uri_config_attr.skip_impl;
    let endpoint_crate = uri_config_attr.crate_path;

    // Extract scheme from struct attributes
    let scheme = extract_scheme(&input.attrs)?;

    // Get struct fields
    let fields = match &input.data {
        Data::Struct(data) => match &data.fields {
            Fields::Named(fields) => &fields.named,
            _ => {
                return Err(syn::Error::new(
                    proc_macro2::Span::call_site(),
                    "UriConfig only supports structs with named fields",
                ));
            }
        },
        _ => {
            return Err(syn::Error::new(
                proc_macro2::Span::call_site(),
                "UriConfig can only be derived for structs",
            ));
        }
    };

    // First pass: collect field info
    #[derive(Clone)]
    enum FieldType {
        Path,
        Param { attr: UriParamAttr },
        DurationFromMs { companion_field: String },
    }

    let mut field_info: Vec<(syn::Ident, Type, FieldType)> = Vec::new();
    let mut path_field_found = false;

    // Collect all field names for Duration companion lookup
    let all_field_names: Vec<String> = fields
        .iter()
        .map(|f| f.ident.as_ref().unwrap().to_string()) // allow-unwrap
        .collect();

    for field in fields {
        let field_name = field.ident.as_ref().unwrap().clone(); // allow-unwrap
        let field_type = field.ty.clone();

        // Check if this is a Duration type that should derive from a companion field
        if is_duration_type(&field.ty) {
            let field_name_str = field_name.to_string();
            let companion_name = format!("{}_ms", field_name_str);

            if all_field_names.contains(&companion_name) {
                field_info.push((
                    field_name,
                    field_type,
                    FieldType::DurationFromMs {
                        companion_field: companion_name,
                    },
                ));
                continue;
            }
            // If no companion, fall through to regular handling (will use FromStr)
        }

        // Check for #[uri_param] attribute
        match parse_uri_param_attr(&field.attrs) {
            Ok(Some(attr)) => {
                field_info.push((field_name, field_type, FieldType::Param { attr }));
            }
            Ok(None) => {
                // No #[uri_param] - this is a path field (only the first one)
                if !path_field_found {
                    path_field_found = true;
                    field_info.push((field_name, field_type, FieldType::Path));
                } else {
                    return Err(syn::Error::new_spanned(
                        field,
                        "only one field can be the path field (first field without #[uri_param])",
                    ));
                }
            }
            Err(e) => {
                return Err(e);
            }
        }
    }

    // Second pass: generate local variable bindings
    let mut bindings = Vec::new();
    let field_names: Vec<_> = field_info.iter().map(|(name, _, _)| name.clone()).collect();

    // Process non-Duration fields first
    for (field_name, field_type, ftype) in &field_info {
        match ftype {
            FieldType::Path => {
                let type_name = get_type_name(field_type);
                match type_name.as_deref() {
                    Some("String") => {
                        bindings.push(quote! {
                            let #field_name = parts.path.clone()
                        });
                    }
                    _ => {
                        let ty = field_type;
                        bindings.push(quote! {
                            let #field_name = parts.path.parse::<#ty>()
                                .map_err(|_| #endpoint_crate::CamelError::InvalidUri(
                                    format!("invalid path value for field: {}", stringify!(#field_name))
                                ))?
                        });
                    }
                }
            }
            FieldType::Param { attr } => {
                let param_name = attr.name.clone().unwrap_or_else(|| field_name.to_string());
                let parsing_code = generate_param_parsing(
                    &param_name,
                    field_name,
                    field_type,
                    attr.default.as_deref(),
                    &endpoint_crate,
                )?;
                bindings.push(parsing_code);
            }
            FieldType::DurationFromMs { .. } => {
                // Process these in the second pass
            }
        }
    }

    // Process Duration fields second (after their companions are bound)
    for (field_name, _field_type, ftype) in &field_info {
        if let FieldType::DurationFromMs { companion_field } = ftype {
            let companion_ident: syn::Ident =
                syn::Ident::new(companion_field, proc_macro2::Span::call_site());
            bindings.push(quote! {
                let #field_name = std::time::Duration::from_millis(#companion_ident)
            });
        }
    }

    let scheme_lit = scheme;

    // Generate the parsing logic (shared between both modes)
    let parsing_logic = quote! {
        // Validate scheme
        if parts.scheme != #scheme_lit {
            return Err(#endpoint_crate::CamelError::InvalidUri(
                format!("expected scheme '{}' but got '{}'", #scheme_lit, parts.scheme)
            ));
        }

        let params = &parts.params;

        #(#bindings);*;

        Ok(Self {
            #(#field_names),*
        })
    };

    // ---- Build uri_options() entries (one per #[uri_param] Param field) ----
    let mut uri_option_entries: Vec<TokenStream> = Vec::new();
    for (field_name, field_type, ftype) in &field_info {
        if let FieldType::Param { attr } = ftype {
            uri_option_entries.push(build_uri_option_entry(
                field_name,
                field_type,
                attr,
                &endpoint_crate,
            )?);
        }
    }

    let uri_options_fn = quote! {
        /// Generated URI option definitions, one per `#[uri_param]` field.
        /// The path field is excluded.
        pub fn uri_options() -> ::std::vec::Vec<#endpoint_crate::UriOption> {
            vec![ #(#uri_option_entries),* ]
        }
    };

    // ---- Conditionally generate metadata() (task 1.4) ----
    let metadata_fn = if uri_config_attr.has_metadata {
        let meta_scheme = uri_config_attr
            .metadata_scheme
            .unwrap_or_else(|| scheme_lit.clone());
        let meta_description = uri_config_attr.metadata_description.unwrap_or_default();
        let sp = uri_config_attr.supports_producer;
        let sc = uri_config_attr.supports_consumer;
        let spc = uri_config_attr.supports_polling_consumer;
        let ss = uri_config_attr.supports_streaming;
        Some(quote! {
            /// Generated `ComponentMetadata`, built from the
            /// `#[uri_config(metadata(..))]` attribute plus the derived
            /// `uri_options()`.
            pub fn metadata() -> #endpoint_crate::ComponentMetadata {
                #endpoint_crate::ComponentMetadata::minimal(#meta_scheme)
                    .with_description(#meta_description)
                    .with_capabilities(#endpoint_crate::ComponentCapabilities {
                        supports_producer: #sp,
                        supports_consumer: #sc,
                        supports_polling_consumer: #spc,
                        supports_streaming: #ss,
                    })
                    .with_uri_options(Self::uri_options())
            }
        })
    } else {
        None
    };

    if skip_impl {
        Ok(quote! {
            impl #struct_name {
                /// Parse URI components into this config.
                /// Call this from your custom `UriConfig::from_components` implementation.
                pub fn parse_uri_components(parts: #endpoint_crate::UriComponents) -> Result<Self, #endpoint_crate::CamelError> {
                    #parsing_logic
                }

                #uri_options_fn
                #metadata_fn
            }
        })
    } else {
        Ok(quote! {
            impl #endpoint_crate::UriConfig for #struct_name {
                fn scheme() -> &'static str {
                    #scheme_lit
                }

                fn from_uri(uri: &str) -> Result<Self, #endpoint_crate::CamelError> {
                    let parts = #endpoint_crate::parse_uri(uri)?;
                    Self::from_components(parts)
                }

                fn from_components(parts: #endpoint_crate::UriComponents) -> Result<Self, #endpoint_crate::CamelError> {
                    let config = Self::parse_uri_components(parts)?;
                    // Call validate to allow custom validation logic
                    config.validate()
                }
            }

            impl #struct_name {
                /// Parse URI components into this config.
                pub fn parse_uri_components(parts: #endpoint_crate::UriComponents) -> Result<Self, #endpoint_crate::CamelError> {
                    #parsing_logic
                }

                #uri_options_fn
                #metadata_fn
            }
        })
    }
}

// ---------------------------------------------------------------------------
// Tests — pure parse/infer unit tests (no macro invocation).
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_attr(src: &str) -> UriParamAttr {
        syn::parse_str::<UriParamAttr>(src).expect("failed to parse uri_param attr")
    }

    #[test]
    fn parse_secret_flag() {
        let attr = parse_attr("secret");
        assert!(attr.secret);
        assert!(!attr.required);
    }

    #[test]
    fn parse_secret_with_other_keys() {
        // Mixed flag + keyvalue form.
        let attr = parse_attr("secret, default = \"x\"");
        assert!(attr.secret);
        assert_eq!(attr.default.as_deref(), Some("x"));
    }

    #[test]
    fn parse_deprecated_key() {
        let attr = parse_attr("deprecated = \"old\"");
        assert_eq!(attr.deprecated.as_deref(), Some("old"));
    }

    #[test]
    fn parse_aliases_array() {
        let attr = parse_attr("aliases = [\"a\", \"b\"]");
        assert_eq!(attr.aliases, vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn parse_unknown_key_still_errors() {
        let res = syn::parse_str::<UriParamAttr>("bogus = 1");
        assert!(res.is_err());
        let msg = res.unwrap_err().to_string();
        assert!(msg.contains("unknown attribute key"), "msg was: {msg}");
    }

    #[test]
    fn parse_required_flag_and_kv() {
        let a = parse_attr("required");
        assert!(a.required);
        let b = parse_attr("required = false");
        assert!(!b.required);
    }

    #[test]
    fn parse_kind_override_captured() {
        let a = parse_attr("kind = \"enum:A,B\"");
        assert_eq!(a.kind_override.as_ref().unwrap().value(), "enum:A,B");
    }

    #[test]
    fn parse_desc_key() {
        let a = parse_attr("desc = \"the period\"");
        assert_eq!(a.desc.as_deref(), Some("the period"));
    }

    fn parse_type(src: &str) -> Type {
        syn::parse_str::<Type>(src).expect("failed to parse type")
    }

    // A dummy crate path for unit-testing inference token output.
    fn test_crate() -> syn::Path {
        syn::parse_quote!(__test_crate)
    }

    fn kind_str(ty: &Type) -> String {
        infer_option_kind(ty, &test_crate()).to_string()
    }

    #[test]
    fn infer_bool() {
        let s = kind_str(&parse_type("bool"));
        assert!(s.contains("Bool"), "{s}");
    }

    #[test]
    fn infer_duration() {
        let s = kind_str(&parse_type("std::time::Duration"));
        assert!(s.contains("Duration"), "{s}");
    }

    #[test]
    fn infer_string() {
        let s = kind_str(&parse_type("String"));
        assert!(s.contains("String"), "{s}");
    }

    #[test]
    fn infer_option_inner_kind() {
        // Option<u32> unwraps to Int.
        let s = kind_str(&parse_type("Option<u32>"));
        assert!(s.contains("Int"), "{s}");
    }

    #[test]
    fn infer_vec_string() {
        let s = kind_str(&parse_type("Vec<String>"));
        assert!(s.contains("List"), "{s}");
        assert!(s.contains("String"), "{s}");
    }

    #[test]
    fn infer_enum_is_string() {
        // An unknown/enum type infers to String — never Enum.
        let s = kind_str(&parse_type("MyMode"));
        assert!(s.contains("String"), "{s}");
    }

    #[test]
    fn infer_ints_and_floats() {
        assert!(kind_str(&parse_type("u64")).contains("Int"));
        assert!(kind_str(&parse_type("i32")).contains("Int"));
        assert!(kind_str(&parse_type("f64")).contains("Float"));
    }

    #[test]
    fn kind_override_enum() {
        let ts = parse_kind_override("enum:A,B", proc_macro2::Span::call_site(), &test_crate())
            .expect("valid override");
        let s = ts.to_string();
        assert!(s.contains("Enum"), "{s}");
        assert!(s.contains("A") && s.contains("B"), "{s}");
    }

    #[test]
    fn kind_override_known_strings() {
        let c = test_crate();
        assert!(
            parse_kind_override("duration", proc_macro2::Span::call_site(), &c)
                .unwrap()
                .to_string()
                .contains("Duration")
        );
        assert!(
            parse_kind_override("bool", proc_macro2::Span::call_site(), &c)
                .unwrap()
                .to_string()
                .contains("Bool")
        );
    }

    #[test]
    fn kind_typo_errors() {
        let res = parse_kind_override("duraton", proc_macro2::Span::call_site(), &test_crate());
        assert!(res.is_err());
    }
}
