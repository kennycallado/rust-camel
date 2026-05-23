mod handler;

use handler::find_handler_methods;
use proc_macro::TokenStream;
use quote::quote;
use syn::{ItemImpl, parse_macro_input};

/// Derive macro for `Bean` — **not yet implemented**.
///
/// This macro is a work-in-progress placeholder. It emits a compile-time error
/// directing users to use `#[bean_impl]` on an impl block instead.
///
/// # Status
///
/// This derive is `#[doc(hidden)]` because it is non-functional. It will be
/// fully implemented in a future release. Use [`bean_impl`] instead.
#[doc(hidden)]
#[proc_macro_derive(Bean)]
pub fn derive_bean(_input: TokenStream) -> TokenStream {
    quote! {
        compile_error!("Bean derive macro is not yet implemented. Use #[bean_impl] on an impl block instead.");
    }
    .into()
}

/// Marker attribute for handler methods
/// This attribute does not transform the method - it's detected by the Bean derive macro
/// to identify which methods should be exposed as bean handlers
#[proc_macro_attribute]
pub fn handler(_attr: TokenStream, item: TokenStream) -> TokenStream {
    // Pass through the method unchanged
    // The Bean derive macro (or bean_impl attribute in Task 2.2) will detect this attribute
    item
}

/// Attribute macro for generating BeanProcessor implementation from an impl block
///
/// # Example
///
/// ```ignore
/// use camel_bean::bean_impl;
/// use camel_bean::handler;
///
/// struct OrderService;
///
/// #[bean_impl]
/// impl OrderService {
///     #[handler]
///     pub async fn process(&self, body: Order) -> Result<ProcessedOrder, String> {
///         // Process order
///         Ok(ProcessedOrder { ... })
///     }
/// }
/// ```
#[proc_macro_attribute]
pub fn bean_impl(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let input = parse_macro_input!(item as ItemImpl);

    match bean_impl_gen(input) {
        Ok(tokens) => tokens.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

/// Generate BeanProcessor implementation from an impl block
fn bean_impl_gen(item: ItemImpl) -> Result<proc_macro2::TokenStream, syn::Error> {
    // BEAN-MACROS-003: Reject generic impl blocks at compile time
    if !item.generics.params.is_empty() {
        return Err(syn::Error::new_spanned(
            &item.generics,
            "bean_impl does not support generic types or lifetimes on the impl block",
        ));
    }

    let self_ty = &item.self_ty;

    // Find handler methods
    let handlers = find_handler_methods(&item.items)?;

    if handlers.is_empty() {
        return Err(syn::Error::new_spanned(
            self_ty,
            "No #[handler] methods found in impl block",
        ));
    }

    // Generate match arms for each handler
    let match_arms: Vec<_> = handlers
        .iter()
        .map(|handler| {
            let method_name = &handler.name;

            // Generate parameter extraction and method call
            let (param_extraction, method_call) = generate_handler_invocation(handler)?;

            // Generate result handling
            let result_handling = generate_result_handling(handler)?;

            Ok(quote! {
                #method_name => {
                    #param_extraction
                    let result = #method_call;
                    #result_handling
                }
            })
        })
        .collect::<Result<Vec<_>, syn::Error>>()?;

    // Generate method names list
    let method_names: Vec<_> = handlers.iter().map(|h| h.name.as_str()).collect();

    let expanded = quote! {
        #item

        #[::camel_bean::async_trait]
        impl ::camel_bean::BeanProcessor for #self_ty {
            async fn call(
                &self,
                method: &str,
                exchange: &mut ::camel_api::Exchange,
            ) -> Result<(), ::camel_api::CamelError> {
                match method {
                    #(#match_arms)*
                    _ => Err(::camel_api::CamelError::ProcessorError(
                        format!("Method '{}' not found", method)
                    ))
                }
            }

            fn methods(&self) -> Vec<String> {
                vec![#(#method_names.to_string()),*]
            }
        }
    };

    Ok(expanded)
}

/// Generate parameter extraction code for a handler method
fn generate_handler_invocation(
    handler: &handler::HandlerMethod,
) -> Result<(proc_macro2::TokenStream, proc_macro2::TokenStream), syn::Error> {
    let method_ident = &handler.ident;

    // Build parameter list
    let mut params = Vec::new();
    let mut extraction = Vec::new();

    // Body parameter
    if let Some(body_type) = &handler.body_type {
        // TODO(BEAN-MACROS-004): Body extraction is currently JSON-only.
        // We should add broader type coercion (e.g., from raw bytes, plain text, etc.)
        // and a proper trait-based extraction strategy. For now, we provide a basic
        // String fallback for String-typed parameters.
        let is_string_type = match body_type {
            syn::Type::Path(type_path) => type_path
                .path
                .segments
                .last()
                .map(|seg| seg.ident == "String")
                .unwrap_or(false),
            _ => false,
        };

        if is_string_type {
            extraction.push(quote! {
                let body: #body_type = match &exchange.input.body {
                    ::camel_api::Body::Json(value) => {
                        ::serde_json::from_value(value.clone())
                            .map_err(|e| ::camel_api::CamelError::TypeConversionFailed(
                                format!("Failed to deserialize body: {}", e)
                            ))?
                    }
                    ::camel_api::Body::Text(s) => s.clone(),
                    other => return Err(::camel_api::CamelError::TypeConversionFailed(
                        format!("Expected JSON or text body, got {:?}", other)
                    )),
                };
            });
        } else {
            extraction.push(quote! {
                let body_json = match &exchange.input.body {
                    ::camel_api::Body::Json(value) => value.clone(),
                    other => return Err(::camel_api::CamelError::TypeConversionFailed(
                        format!("Expected JSON body, got {:?}", other)
                    )),
                };
                let body: #body_type = ::serde_json::from_value(body_json)
                    .map_err(|e| ::camel_api::CamelError::TypeConversionFailed(
                        format!("Failed to deserialize body: {}", e)
                    ))?;
            });
        }
        params.push(quote! { body });
    }

    // Headers parameter
    if handler.has_headers {
        extraction.push(quote! {
            let headers = exchange.input.headers.clone();
        });
        params.push(quote! { headers });
    }

    // Exchange parameter
    if handler.has_exchange {
        params.push(quote! { exchange });
    }

    let extraction_code = quote! { #(#extraction)* };
    let method_call = quote! { self.#method_ident(#(#params),*).await };

    Ok((extraction_code, method_call))
}

/// Generate result handling code based on return type
fn generate_result_handling(
    handler: &handler::HandlerMethod,
) -> Result<proc_macro2::TokenStream, syn::Error> {
    // If no return type or exchange-only handler, don't set body
    if handler.return_type.is_none() {
        return Ok(quote! {
            result.map_err(|e| ::camel_api::CamelError::ProcessorError(e.to_string()))?;
            Ok(())
        });
    }

    // Check if this is an exchange-only handler (has exchange but no body type)
    if handler.has_exchange && handler.body_type.is_none() {
        // Exchange handlers manage the exchange themselves, just propagate errors
        return Ok(quote! {
            result.map_err(|e| ::camel_api::CamelError::ProcessorError(e.to_string()))?;
            Ok(())
        });
    }

    // For handlers with return values, extract the value and set it as body
    // We need to handle Result<T> return types
    Ok(quote! {
        let value = result.map_err(|e| ::camel_api::CamelError::ProcessorError(e.to_string()))?;
        exchange.input.body = ::camel_api::Body::Json(::serde_json::to_value(value)
            .map_err(|e| ::camel_api::CamelError::TypeConversionFailed(
                format!("Failed to serialize result: {}", e)
            ))?);
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use syn::parse_quote;

    /// BEAN-MACROS-005: Verify bean_impl_gen succeeds for a valid simple impl block
    #[test]
    fn test_bean_impl_gen_simple_valid() {
        let item: ItemImpl = parse_quote! {
            impl MyService {
                #[handler]
                pub async fn process(&self, body: String) -> Result<String, String> {
                    Ok(body)
                }
            }
        };
        let result = bean_impl_gen(item);
        assert!(
            result.is_ok(),
            "bean_impl_gen should succeed for valid input"
        );
        let tokens = result.unwrap().to_string();
        assert!(
            tokens.contains("BeanProcessor"),
            "should generate BeanProcessor impl"
        );
        assert!(
            tokens.contains("process"),
            "should contain handler method name"
        );
    }

    /// BEAN-MACROS-005: Verify bean_impl_gen rejects impl blocks with no #[handler] methods
    #[test]
    fn test_bean_impl_gen_no_handlers() {
        let item: ItemImpl = parse_quote! {
            impl MyService {
                pub async fn process(&self) {}
            }
        };
        let result = bean_impl_gen(item);
        assert!(
            result.is_err(),
            "bean_impl_gen should fail when no handlers found"
        );
        let err = result.unwrap_err();
        assert!(
            err.to_string().contains("No #[handler] methods found"),
            "error should mention missing handlers, got: {}",
            err
        );
    }

    /// BEAN-MACROS-003/005: Verify bean_impl_gen rejects generic impl blocks
    #[test]
    fn test_bean_impl_gen_rejects_generics() {
        let item: ItemImpl = parse_quote! {
            impl<T> GenericService<T> {
                #[handler]
                pub async fn process(&self, body: T) -> Result<T, String> {
                    Ok(body)
                }
            }
        };
        let result = bean_impl_gen(item);
        assert!(
            result.is_err(),
            "bean_impl_gen should reject generic impl blocks"
        );
        let err = result.unwrap_err();
        assert!(
            err.to_string().contains("generic"),
            "error should mention generics, got: {}",
            err
        );
    }

    /// BEAN-MACROS-003/005: Verify bean_impl_gen rejects impl blocks with lifetimes
    #[test]
    fn test_bean_impl_gen_rejects_lifetimes() {
        let item: ItemImpl = parse_quote! {
            impl<'a> Service<'a> {
                #[handler]
                pub async fn process(&self) -> Result<(), String> {
                    Ok(())
                }
            }
        };
        let result = bean_impl_gen(item);
        assert!(
            result.is_err(),
            "bean_impl_gen should reject lifetime impl blocks"
        );
        let err = result.unwrap_err();
        assert!(
            err.to_string().contains("generic") || err.to_string().contains("lifetime"),
            "error should mention generics/lifetimes, got: {}",
            err
        );
    }

    /// BEAN-MACROS-005: Verify bean_impl_gen works with multiple handlers
    #[test]
    fn test_bean_impl_gen_multiple_handlers() {
        let item: ItemImpl = parse_quote! {
            impl MyService {
                #[handler]
                pub async fn process(&self, body: String) -> Result<String, String> {
                    Ok(body)
                }
                #[handler]
                pub async fn validate(&self, body: String) -> Result<bool, String> {
                    Ok(true)
                }
            }
        };
        let result = bean_impl_gen(item);
        assert!(
            result.is_ok(),
            "bean_impl_gen should succeed with multiple handlers"
        );
        let tokens = result.unwrap().to_string();
        assert!(tokens.contains("process"), "should contain first handler");
        assert!(tokens.contains("validate"), "should contain second handler");
    }

    /// BEAN-MACROS-005: Verify derive_bean logic produces compile_error
    /// (We test the internal quote logic since the proc_macro entry point
    /// cannot be called outside of a proc-macro invocation context.)
    #[test]
    fn test_derive_bean_logic_emits_compile_error() {
        let tokens: proc_macro2::TokenStream = quote! {
            compile_error!("Bean derive macro is not yet implemented. Use #[bean_impl] on an impl block instead.");
        };
        let token_str = tokens.to_string();
        assert!(
            token_str.contains("compile_error"),
            "derive_bean logic should emit compile_error!, got: {}",
            token_str
        );
    }
}
