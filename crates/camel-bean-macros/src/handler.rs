use syn::{Attribute, Error, ImplItem, ImplItemFn, Result, Type};

/// Represents a parsed handler method with its metadata
#[derive(Debug, Clone)]
pub struct HandlerMethod {
    /// Method name
    pub name: String,
    /// Method identifier for code generation
    pub ident: syn::Ident,
    /// Has a parameter named "body" and its type
    pub body_type: Option<Type>,
    /// Has a parameter named "headers"
    pub has_headers: bool,
    /// Has a parameter named "exchange"
    pub has_exchange: bool,
    /// Return type (None for unit return)
    pub return_type: Option<Type>,
}

/// Find all methods with #[handler] attribute in an impl block
pub fn find_handler_methods(items: &[ImplItem]) -> Result<Vec<HandlerMethod>> {
    let mut handlers = Vec::new();
    let mut seen_names = std::collections::HashSet::new();

    for item in items {
        if let ImplItem::Fn(method) = item
            && has_handler_attribute(&method.attrs)
        {
            let handler = parse_handler_method(method)?;

            // Check for duplicate method names
            if !seen_names.insert(handler.name.clone()) {
                return Err(Error::new_spanned(
                    &method.sig,
                    format!("Duplicate handler method name: '{}'", handler.name),
                ));
            }

            handlers.push(handler);
        }
    }

    Ok(handlers)
}

/// Check if method has #[handler] or #[camel_bean::handler] attribute
fn has_handler_attribute(attrs: &[Attribute]) -> bool {
    attrs.iter().any(|attr| {
        let path = attr.path();

        // Check if path is just "handler"
        if path.is_ident("handler") {
            return true;
        }

        // Check if path ends with "::handler" (e.g., camel_bean::handler)
        let segments = &path.segments;
        if let Some(last) = segments.last()
            && last.ident == "handler"
        {
            return true;
        }

        false
    })
}

/// Parse a handler method and extract metadata
fn parse_handler_method(method: &ImplItemFn) -> Result<HandlerMethod> {
    let name = method.sig.ident.to_string();
    let ident = method.sig.ident.clone();
    let mut body_type = None;
    let mut has_headers = false;
    let mut has_exchange = false;
    let mut has_self = false;

    // Track parameter names to detect duplicates
    let mut param_names = std::collections::HashSet::new();

    for input in &method.sig.inputs {
        match input {
            syn::FnArg::Receiver(receiver) => {
                // Check for &self
                if receiver.reference.is_some() {
                    has_self = true;
                } else {
                    return Err(Error::new_spanned(
                        receiver,
                        "Handler methods must use &self, not self",
                    ));
                }
            }
            syn::FnArg::Typed(pat_type) => {
                let param_name = match &*pat_type.pat {
                    syn::Pat::Ident(pat_ident) => pat_ident.ident.to_string(),
                    _ => continue,
                };

                // Check for duplicate parameter names
                if !param_names.insert(param_name.clone()) {
                    return Err(Error::new_spanned(
                        pat_type,
                        format!("Duplicate parameter name: '{}'", param_name),
                    ));
                }

                // Detect special parameters by name
                if param_name == "body" {
                    body_type = Some((*pat_type.ty).clone());
                } else if param_name == "headers" {
                    has_headers = true;
                } else if param_name == "exchange" {
                    has_exchange = true;
                }
                // Note: We allow other parameters, they just won't get special binding
            }
        }
    }

    // Validate method has &self
    if !has_self {
        return Err(Error::new_spanned(
            &method.sig,
            "Handler methods must have &self as first parameter",
        ));
    }

    // Validate method is async
    if method.sig.asyncness.is_none() {
        return Err(Error::new_spanned(
            &method.sig,
            "Handler methods must be async",
        ));
    }

    // Extract return type
    let return_type = match &method.sig.output {
        syn::ReturnType::Default => None, // Unit return
        syn::ReturnType::Type(_, ty) => Some((**ty).clone()),
    };

    Ok(HandlerMethod {
        name,
        ident,
        body_type,
        has_headers,
        has_exchange,
        return_type,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use syn::parse_quote;

    #[test]
    fn test_has_handler_attribute_simple() {
        let method: ImplItemFn = parse_quote! {
            #[handler]
            pub async fn process(&self) {}
        };
        assert!(has_handler_attribute(&method.attrs));
    }

    #[test]
    fn test_has_handler_attribute_qualified() {
        let method: ImplItemFn = parse_quote! {
            #[camel_bean::handler]
            pub async fn process(&self) {}
        };
        assert!(has_handler_attribute(&method.attrs));
    }

    #[test]
    fn test_has_handler_attribute_none() {
        let method: ImplItemFn = parse_quote! {
            pub async fn process(&self) {}
        };
        assert!(!has_handler_attribute(&method.attrs));
    }

    #[test]
    fn test_parse_handler_method_body_only() {
        let method: ImplItemFn = parse_quote! {
            #[handler]
            pub async fn process(&self, body: String) {}
        };
        let result = parse_handler_method(&method).unwrap();
        assert_eq!(result.name, "process");
        assert!(result.body_type.is_some());
        assert!(!result.has_headers);
        assert!(!result.has_exchange);
    }

    #[test]
    fn test_parse_handler_method_multiple_params() {
        let method: ImplItemFn = parse_quote! {
            #[handler]
            pub async fn process(&self, body: Order, headers: Headers) {}
        };
        let result = parse_handler_method(&method).unwrap();
        assert!(result.body_type.is_some());
        assert!(result.has_headers);
        assert!(!result.has_exchange);
    }

    #[test]
    fn test_parse_handler_method_exchange() {
        let method: ImplItemFn = parse_quote! {
            #[handler]
            pub async fn process(&self, exchange: &mut Exchange) {}
        };
        let result = parse_handler_method(&method).unwrap();
        assert!(result.body_type.is_none());
        assert!(!result.has_headers);
        assert!(result.has_exchange);
    }

    #[test]
    fn test_parse_handler_method_non_async_error() {
        let method: ImplItemFn = parse_quote! {
            #[handler]
            pub fn process(&self) {}
        };
        let result = parse_handler_method(&method);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("must be async"));
    }

    #[test]
    fn test_parse_handler_method_missing_self_error() {
        let method: ImplItemFn = parse_quote! {
            #[handler]
            pub async fn process() {}
        };
        let result = parse_handler_method(&method);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("&self"));
    }

    #[test]
    fn test_find_handler_methods_multiple() {
        let items: Vec<ImplItem> = vec![
            parse_quote! {
                #[handler]
                pub async fn process(&self, body: String) {}
            },
            parse_quote! {
                #[handler]
                pub async fn validate(&self, body: String) {}
            },
            parse_quote! {
                pub fn helper(&self) {}
            },
        ];
        let result = find_handler_methods(&items).unwrap();
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].name, "process");
        assert_eq!(result[1].name, "validate");
    }

    #[test]
    fn test_find_handler_methods_duplicate_name_error() {
        let items: Vec<ImplItem> = vec![
            parse_quote! {
                #[handler]
                pub async fn process(&self, body: String) {}
            },
            parse_quote! {
                #[handler]
                pub async fn process(&self, body: i32) {}
            },
        ];
        let result = find_handler_methods(&items);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Duplicate"));
    }
}
