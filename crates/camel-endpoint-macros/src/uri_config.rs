use proc_macro2::TokenStream;
use quote::quote;
use syn::{
    Data, DeriveInput, Fields, Lit, Meta, Token, Type, TypePath, parse::Parse, parse::ParseStream,
    punctuated::Punctuated,
};

/// Parsed `#[uri_param]` attribute
struct UriParamAttr {
    /// Custom parameter name (if specified)
    name: Option<String>,
    /// Default value (if specified)
    default: Option<String>,
}

/// Parse a single key=value pair
struct KeyValue {
    key: syn::Ident,
    value: Lit,
}

impl Parse for KeyValue {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let key: syn::Ident = input.parse()?;
        input.parse::<Token![=]>()?;
        let value: Lit = input.parse()?;
        Ok(KeyValue { key, value })
    }
}

impl Parse for UriParamAttr {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let mut name = None;
        let mut default = None;

        if input.is_empty() {
            return Ok(UriParamAttr { name, default });
        }

        // Parse comma-separated key=value pairs
        let pairs: Punctuated<KeyValue, Token![,]> =
            input.parse_terminated(KeyValue::parse, Token![,])?;

        for pair in pairs {
            let key_str = pair.key.to_string();
            if let Lit::Str(lit_str) = pair.value {
                let str_val = lit_str.value();
                match key_str.as_str() {
                    "name" => name = Some(str_val),
                    "default" => default = Some(str_val),
                    _ => {
                        return Err(syn::Error::new_spanned(
                            pair.key,
                            format!("unknown attribute key: {}", key_str),
                        ));
                    }
                }
            } else {
                return Err(syn::Error::new_spanned(
                    pair.value,
                    "expected a string literal",
                ));
            }
        }

        Ok(UriParamAttr { name, default })
    }
}

/// Extract the URI scheme from struct attributes
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

/// Parse a `#[uri_param]` attribute from field attributes
fn parse_uri_param_attr(attrs: &[syn::Attribute]) -> syn::Result<Option<UriParamAttr>> {
    for attr in attrs {
        if attr.path().is_ident("uri_param") {
            // Check if there are any tokens (i.e., parentheses present)
            // If the attribute is just `#[uri_param]` with no parens, return empty UriParamAttr
            match &attr.meta {
                Meta::Path(_) => {
                    // No parentheses - just #[uri_param]
                    return Ok(Some(UriParamAttr {
                        name: None,
                        default: None,
                    }));
                }
                Meta::List(list) => {
                    // Has parentheses - parse the contents
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

struct UriConfigAttr {
    skip_impl: bool,
    crate_path: syn::Path,
}

fn parse_uri_config_attr(attrs: &[syn::Attribute]) -> syn::Result<UriConfigAttr> {
    let mut skip_impl = false;
    let mut crate_path: Option<syn::Path> = None;

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
    })
}

/// Get the type name as a string (for simple type matching)
fn get_type_name(ty: &Type) -> Option<String> {
    if let Type::Path(TypePath { path, .. }) = ty {
        // Get the last segment (handles qualified paths)
        let segment = path.segments.last()?;
        Some(segment.ident.to_string())
    } else {
        None
    }
}

/// Check if a type is std::time::Duration
fn is_duration_type(ty: &Type) -> bool {
    if let Type::Path(TypePath { path, .. }) = ty {
        // Handle both `Duration` and `std::time::Duration`
        let segments: Vec<_> = path.segments.iter().map(|s| s.ident.to_string()).collect();

        // Direct "Duration" or qualified "std::time::Duration"
        segments.last().map(|s| s == "Duration").unwrap_or(false)
    } else {
        false
    }
}

/// Check if a type is Option<T>
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

/// Generate code to parse a value from params into a local variable
/// Returns (variable_name, binding_code) where binding_code assigns to the variable
fn generate_param_parsing(
    param_name: &str,
    field_name: &syn::Ident,
    ty: &Type,
    default: Option<&str>,
    endpoint_crate: &syn::Path,
) -> TokenStream {
    let type_name = get_type_name(ty);
    let inner_type = is_option_type(ty);

    // Handle Option<T>
    if let Some(inner_ty) = &inner_type {
        let inner_type_name = get_type_name(inner_ty);

        return match inner_type_name.as_deref() {
            Some("String") => quote! {
                let #field_name = params.get(#param_name).cloned()
            },
            Some("bool") => quote! {
                let #field_name = params.get(#param_name)
                    .map(|v| v == "true")
            },
            Some("u64") | Some("u32") | Some("usize") | Some("i64") | Some("i32")
            | Some("isize") => quote! {
                let #field_name = params.get(#param_name)
                    .and_then(|v| v.parse().ok())
            },
            _ => quote! {
                let #field_name = params.get(#param_name)
                    .map(|v| v.parse().ok())
                    .flatten()
            },
        };
    }

    // Handle non-Option types
    match type_name.as_deref() {
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
                let default_bool = default_val == "true";
                quote! {
                    let #field_name = params.get(#param_name)
                        .map(|v| v == "true")
                        .unwrap_or(#default_bool)
                }
            } else {
                // Require the param instead of silent false default
                quote! {
                    let #field_name = params.get(#param_name)
                        .map(|v| v == "true")
                        .ok_or_else(|| #endpoint_crate::CamelError::InvalidUri(
                            format!("missing required parameter: {}", #param_name)
                        ))?
                }
            }
        }
        Some("u64") => {
            if let Some(default_val) = default {
                let default_num: u64 = default_val.parse().unwrap_or(0);
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
                let default_num: u32 = default_val.parse().unwrap_or(0);
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
                let default_num: usize = default_val.parse().unwrap_or(0);
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
                let default_num: i64 = default_val.parse().unwrap_or(0);
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
                let default_num: i32 = default_val.parse().unwrap_or(0);
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
                    let #field_name = params.get(#param_name)
                        .map(|v| v.parse::<#ty>().unwrap_or_else(|_| #default_val.parse().unwrap()))
                        .unwrap_or_else(|| #default_val.parse().unwrap())
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
    }
}

pub fn impl_uri_config(input: &DeriveInput) -> TokenStream {
    let struct_name = &input.ident;

    let uri_config_attr = match parse_uri_config_attr(&input.attrs) {
        Ok(a) => a,
        Err(e) => return e.to_compile_error(),
    };

    let skip_impl = uri_config_attr.skip_impl;
    let endpoint_crate = uri_config_attr.crate_path;

    // Extract scheme from struct attributes
    let scheme = match extract_scheme(&input.attrs) {
        Ok(s) => s,
        Err(e) => return e.to_compile_error(),
    };

    // Get struct fields
    let fields = match &input.data {
        Data::Struct(data) => match &data.fields {
            Fields::Named(fields) => &fields.named,
            _ => {
                return syn::Error::new(
                    proc_macro2::Span::call_site(),
                    "UriConfig only supports structs with named fields",
                )
                .to_compile_error();
            }
        },
        _ => {
            return syn::Error::new(
                proc_macro2::Span::call_site(),
                "UriConfig can only be derived for structs",
            )
            .to_compile_error();
        }
    };

    // First pass: collect field info
    #[derive(Clone)]
    enum FieldType {
        Path,
        Param {
            param_name: String,
            default: Option<String>,
        },
        DurationFromMs {
            companion_field: String,
        },
    }

    let mut field_info: Vec<(syn::Ident, Type, FieldType)> = Vec::new();
    let mut path_field_found = false;

    // Collect all field names for Duration companion lookup
    let all_field_names: Vec<String> = fields
        .iter()
        .map(|f| f.ident.as_ref().unwrap().to_string())
        .collect();

    for field in fields {
        let field_name = field.ident.as_ref().unwrap().clone();
        let field_type = field.ty.clone();

        // Check if this is a Duration type that should derive from a companion field
        if is_duration_type(&field.ty) {
            let field_name_str = field_name.to_string();
            let companion_name = format!("{}_ms", field_name_str);

            // Check if companion field exists
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
                // This is a parameter field
                let param_name = attr.name.clone().unwrap_or_else(|| field_name.to_string());
                field_info.push((
                    field_name,
                    field_type,
                    FieldType::Param {
                        param_name,
                        default: attr.default,
                    },
                ));
            }
            Ok(None) => {
                // No #[uri_param] - this is a path field (only the first one)
                if !path_field_found {
                    path_field_found = true;
                    field_info.push((field_name, field_type, FieldType::Path));
                } else {
                    // Additional path field without attribute - error
                    return syn::Error::new_spanned(
                        field,
                        "only one field can be the path field (first field without #[uri_param])",
                    )
                    .to_compile_error();
                }
            }
            Err(e) => {
                return e.to_compile_error();
            }
        }
    }

    // Second pass: generate local variable bindings
    // We need to ensure companion fields are processed before Duration fields
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
                        // Try to parse the path
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
            FieldType::Param {
                param_name,
                default,
            } => {
                let parsing_code = generate_param_parsing(
                    param_name,
                    field_name,
                    field_type,
                    default.as_deref(),
                    &endpoint_crate,
                );
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

    if skip_impl {
        // Generate an inherent method for parsing, user implements UriConfig manually
        quote! {
            impl #struct_name {
                /// Parse URI components into this config.
                /// Call this from your custom `UriConfig::from_components` implementation.
                pub fn parse_uri_components(parts: #endpoint_crate::UriComponents) -> Result<Self, #endpoint_crate::CamelError> {
                    #parsing_logic
                }
            }
        }
    } else {
        // Generate full UriConfig trait implementation
        quote! {
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
            }
        }
    }
}
