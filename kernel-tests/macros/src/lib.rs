// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Register bare-metal test functions with Rust's custom test harness.

#![forbid(unsafe_code)]

use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::{ItemFn, LitStr, Token, parse_macro_input, punctuated::Punctuated};

/// Collect a synchronous `fn()` as a named test case.
///
/// Optional string arguments identify expected fatal diagnostics. The
/// host runner requires all those diagnostics after the case starts instead of a
/// normal return. The test crate must enable `custom_test_frameworks`.
#[proc_macro_attribute]
pub fn ktest(arguments: TokenStream, item: TokenStream) -> TokenStream {
    let messages =
        parse_macro_input!(arguments with Punctuated::<LitStr, Token![,]>::parse_terminated);
    for message in &messages {
        if message.value().is_empty() || message.value().contains(['\n', '\r', '\t']) {
            return syn::Error::new_spanned(
                message,
                "expected diagnostic must be nonempty and on one line",
            )
            .to_compile_error()
            .into();
        }
    }
    let function = parse_macro_input!(item as ItemFn);
    let signature = &function.sig;
    if signature.asyncness.is_some()
        || signature.unsafety.is_some()
        || signature.abi.is_some()
        || !signature.inputs.is_empty()
        || !signature.generics.params.is_empty()
        || !matches!(signature.output, syn::ReturnType::Default)
    {
        return syn::Error::new_spanned(signature, "#[ktest] requires a safe synchronous fn()")
            .to_compile_error()
            .into();
    }
    let name = &signature.ident;
    let descriptor = format_ident!("__KTEST_{name}");
    quote! {
        #function

        #[test_case]
        #[allow(non_upper_case_globals)]
        const #descriptor: (&str, fn(), &[&str]) = (
            concat!(module_path!(), "::", stringify!(#name)),
            #name,
            &[#messages],
        );
    }
    .into()
}
