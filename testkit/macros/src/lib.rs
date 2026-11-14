// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Register functional and performance cases with Rust's custom test harness.

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
    if let Err(error) = validate(&function, 0, "#[ktest] requires a safe synchronous fn()") {
        return error.to_compile_error().into();
    }
    let name = &function.sig.ident;
    let descriptor = format_ident!("__KTEST_{name}");
    quote! {
        #function

        #[test_case]
        #[allow(non_upper_case_globals)]
        const #descriptor: ::testkit::Case = ::testkit::Case {
            name: concat!(module_path!(), "::", stringify!(#name)),
            kind: ::testkit::CaseKind::Test { run: #name, expected: &[#messages] },
        };
    }
    .into()
}

/// Collect a safe synchronous `fn(&mut testkit::perf::Perf)` as a performance case.
#[proc_macro_attribute]
pub fn kperf(arguments: TokenStream, item: TokenStream) -> TokenStream {
    let function = parse_macro_input!(item as ItemFn);
    if !arguments.is_empty() {
        return syn::Error::new_spanned(&function.sig, "#[kperf] takes no arguments")
            .to_compile_error()
            .into();
    }
    if let Err(error) = validate(
        &function,
        1,
        "#[kperf] requires a safe synchronous fn(&mut testkit::perf::Perf)",
    ) {
        return error.to_compile_error().into();
    }
    let name = &function.sig.ident;
    let descriptor = format_ident!("__KPERF_{name}");
    quote! {
        #function

        #[test_case]
        #[allow(non_upper_case_globals)]
        const #descriptor: ::testkit::Case = ::testkit::Case {
            name: concat!(module_path!(), "::", stringify!(#name)),
            kind: ::testkit::CaseKind::Perf(#name),
        };
    }
    .into()
}

fn validate(function: &ItemFn, inputs: usize, message: &str) -> syn::Result<()> {
    let signature = &function.sig;
    if signature.asyncness.is_some()
        || signature.unsafety.is_some()
        || signature.abi.is_some()
        || signature.inputs.len() != inputs
        || !signature.generics.params.is_empty()
        || !matches!(signature.output, syn::ReturnType::Default)
    {
        return Err(syn::Error::new_spanned(signature, message));
    }
    Ok(())
}
