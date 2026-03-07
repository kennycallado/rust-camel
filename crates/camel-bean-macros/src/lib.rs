mod handler;

use handler::find_handler_methods;
use proc_macro::TokenStream;
use quote::quote;
use syn::{DeriveInput, ItemImpl, parse_macro_input};

/// Derive macro for BeanProcessor trait implementation
/// This is a placeholder - Task 2.2 will complete the implementation
#[proc_macro_derive(Bean)]
pub fn derive_bean(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);

    // TODO: Implementation in next task
    let name = &input.ident;

    let expanded = quote! {
        // Placeholder implementation
        impl camel_bean::BeanProcessor for #name {
            async fn call(
                &self,
                method: &str,
                exchange: &mut camel_api::Exchange,
            ) -> Result<(), camel_api::CamelError> {
                unimplemented!("Bean derive macro requires impl block with #[handler] methods")
            }

            fn methods(&self) -> Vec<&'static str> {
                vec![]
            }
        }
    };

    TokenStream::from(expanded)
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
        Ok(tokens) => tokens,
        Err(err) => err.to_compile_error().into(),
    }
}

/// Generate BeanProcessor implementation from an impl block
fn bean_impl_gen(item: ItemImpl) -> Result<TokenStream, syn::Error> {
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

            fn methods(&self) -> Vec<&'static str> {
                vec![#(#method_names),*]
            }
        }
    };

    Ok(TokenStream::from(expanded))
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
        extraction.push(quote! {
            let body_json = match &exchange.input.body {
                ::camel_api::Body::Json(value) => value.clone(),
                _ => return Err(::camel_api::CamelError::TypeConversionFailed(
                    "Expected JSON body".to_string()
                )),
            };
            let body: #body_type = serde_json::from_value(body_json)
                .map_err(|e| ::camel_api::CamelError::TypeConversionFailed(
                    format!("Failed to deserialize body: {}", e)
                ))?;
        });
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
        exchange.input.body = ::camel_api::Body::Json(serde_json::to_value(value)
            .map_err(|e| ::camel_api::CamelError::TypeConversionFailed(
                format!("Failed to serialize result: {}", e)
            ))?);
        Ok(())
    })
}
