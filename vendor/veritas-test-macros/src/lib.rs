//! Compile-time validation for Rust evidence metadata attributes.
//!
//! Attributes retain test execution behavior unchanged. Native discovery reads
//! metadata from source; this crate only gives authors immediate diagnostics.

use proc_macro2::TokenStream;
use quote::ToTokens as _;
use syn::{ItemFn, LitStr, Token, parse::Parser as _, punctuated::Punctuated};

/// Attach one or more Veritas claim IDs to a direct test function.
#[proc_macro_attribute]
pub fn claims(
    arguments: proc_macro::TokenStream,
    input: proc_macro::TokenStream,
) -> proc_macro::TokenStream {
    expand(arguments.into(), input.into(), ValueKind::Claim).into()
}

/// Attach one or more configuration-relative dependency paths to a test.
#[proc_macro_attribute]
pub fn depends(
    arguments: proc_macro::TokenStream,
    input: proc_macro::TokenStream,
) -> proc_macro::TokenStream {
    expand(arguments.into(), input.into(), ValueKind::Dependency).into()
}

#[derive(Clone, Copy)]
enum ValueKind {
    Claim,
    Dependency,
}

fn expand(arguments: TokenStream, input: TokenStream, kind: ValueKind) -> TokenStream {
    match try_expand(arguments, input, kind) {
        Ok(expanded) => expanded,
        Err(error) => error.into_compile_error(),
    }
}

fn try_expand(
    arguments: TokenStream,
    input: TokenStream,
    kind: ValueKind,
) -> syn::Result<TokenStream> {
    validate_values(arguments, kind)?;
    let function = syn::parse2::<ItemFn>(input).map_err(|_| {
        syn::Error::new(
            proc_macro2::Span::call_site(),
            "Veritas metadata may annotate only a direct test function",
        )
    })?;
    if function.sig.asyncness.is_some() {
        return Err(syn::Error::new_spanned(
            &function.sig.ident,
            "Veritas metadata must expand after the test attribute; place it directly above the function signature",
        ));
    }
    if !function.attrs.iter().any(is_test_attribute) {
        return Err(syn::Error::new_spanned(
            &function.sig.ident,
            "Veritas metadata requires a direct test attribute",
        ));
    }
    Ok(function.into_token_stream())
}

fn is_test_attribute(attribute: &syn::Attribute) -> bool {
    attribute
        .path()
        .segments
        .last()
        .is_some_and(|segment| segment.ident == "test")
}

fn validate_values(arguments: TokenStream, kind: ValueKind) -> syn::Result<()> {
    let values = Punctuated::<LitStr, Token![,]>::parse_terminated.parse2(arguments)?;
    if values.is_empty() {
        return Err(syn::Error::new(
            proc_macro2::Span::call_site(),
            "Veritas metadata requires at least one string literal",
        ));
    }
    for value in values {
        let text = value.value();
        let valid = match kind {
            ValueKind::Claim => valid_claim_id(&text),
            ValueKind::Dependency => valid_dependency(&text),
        };
        if !valid {
            let message = match kind {
                ValueKind::Claim => "Veritas claim must be a namespaced lowercase kebab-case ID",
                ValueKind::Dependency => "Veritas dependency must be a configuration-relative path",
            };
            return Err(syn::Error::new(value.span(), message));
        }
    }
    Ok(())
}

fn valid_dependency(value: &str) -> bool {
    !value.is_empty()
        && !value.contains(['\n', '\r', '\0'])
        && !value.starts_with(['/', '\\'])
        && !value
            .split(['/', '\\'])
            .any(|part| part.is_empty() || part == "." || part == "..")
}

fn valid_claim_id(value: &str) -> bool {
    let Some((namespace, name)) = value.split_once('/') else {
        return false;
    };
    !name.contains('/') && valid_component(namespace) && valid_component(name)
}

fn valid_component(value: &str) -> bool {
    let mut segments = value.split('-');
    let Some(first) = segments.next() else {
        return false;
    };
    starts_lowercase(first) && segments.all(nonempty_lowercase_or_digit)
}

fn starts_lowercase(value: &str) -> bool {
    value.as_bytes().first().is_some_and(u8::is_ascii_lowercase)
        && value.as_bytes()[1..]
            .iter()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
}

fn nonempty_lowercase_or_digit(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use quote::quote;

    use super::{ValueKind, try_expand, valid_claim_id, valid_dependency};

    #[test]
    fn validates_claim_ids() {
        assert!(valid_claim_id("compiler/emits-start"));
        assert!(!valid_claim_id("compiler/emits_start"));
    }

    #[test]
    fn validates_dependency_paths() {
        assert!(valid_dependency("tests/fixtures/retry.json"));
        assert!(!valid_dependency("../secret"));
        assert!(!valid_dependency("/absolute"));
    }

    #[test]
    fn rejects_malformed_values_and_unsupported_placements() {
        for arguments in [quote!(), quote!("not-namespaced")] {
            assert!(
                try_expand(
                    arguments,
                    quote!(
                        #[test]
                        fn works() {}
                    ),
                    ValueKind::Claim
                )
                .is_err()
            );
        }
        assert!(
            try_expand(
                quote!("spec/one"),
                quote!(
                    fn helper() {}
                ),
                ValueKind::Claim,
            )
            .is_err()
        );
        assert!(
            try_expand(
                quote!("spec/one"),
                quote!(
                    struct NotATest;
                ),
                ValueKind::Claim,
            )
            .is_err()
        );
    }
}
