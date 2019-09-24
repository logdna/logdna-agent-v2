#![recursion_limit = "128"]

extern crate proc_macro;

use proc_macro::TokenStream;
use hashbrown::HashMap;

use proc_macro2::{Ident, TokenStream as TokenStream2, Span};
use syn::{ItemStruct, Lit, Meta, NestedMeta};

use quote::{quote, TokenStreamExt, ToTokens};

#[proc_macro_attribute]
pub fn env_config(_attr: TokenStream, item: TokenStream) -> TokenStream {
    // convert to proc_macro2 stream
    let item = TokenStream2::from(item);
    // the new token stream that will be returned
    let mut new_item = TokenStream2::new();
    // parse the incoming token stream into a Struct syntax tree
    let mut input: ItemStruct = syn::parse2(item).unwrap();
    let name = input.ident.clone();
    // the field to env var(s) mapping
    // e.g foo => FOO, FOO_OLD, FOO_ETC
    let mut field_map = HashMap::new();
    let mut default_map = HashMap::new();
    let mut example_map = HashMap::new();
    // iterate over all field of the struct
    for field in input.fields.iter_mut() {
        // iterate over all the attributes in for the field
        let field_name = field.clone().ident.unwrap();
        field.attrs.retain(|attr| {
            // parse the attribute into a Meta::List
            if let Meta::List(list) = attr.parse_meta().unwrap() {
                // make sure we are only working with env attributes
                if list.ident == "env" {
                    // the list of env var name we parsed from the attribute
                    let mut envs = Vec::new();
                    // collect all the env var names into envs
                    for nested in list.nested {
                        if let NestedMeta::Meta(meta) = nested {
                            envs.push(meta.name().to_string())
                        }
                    }
                    // insert the field and env var mapping
                    field_map.insert(field_name.clone(), envs);
                    // remove the attr so the struct compiles
                    return false;
                }

                if list.ident == "default" {
                    // this attribute has only one nested meta
                    if let Some(pair) = list.nested.first() {
                        if let NestedMeta::Literal(lit) = pair.into_value() {
                            // insert the default value for that field
                            default_map.insert(field_name.clone(), lit.clone());
                            return false;
                        }
                    }
                }

                if list.ident == "example" {
                    // this attribute has only one nested meta
                    if let Some(pair) = list.nested.first() {
                        if let NestedMeta::Literal(lit) = pair.into_value() {
                            // insert the default value for that field
                            example_map.insert(field_name.clone(), lit.clone());
                            return false;
                        }
                    }
                }
            }

            true
        });
    }

    input.to_tokens(&mut new_item);
    new_item.append_all(generate_env_vars(&name, &field_map, &default_map));
    new_item.append_all(generate_tests(&name, &field_map, &default_map, &example_map));
    TokenStream::from(new_item)
}

fn generate_env_vars(
    name: &Ident,
    field_map: &HashMap<Ident, Vec<String>>,
    default_map: &HashMap<Ident, Lit>,
) -> TokenStream2 {
    let mut fields = TokenStream2::new();

    for (field, env_vars) in field_map {
        let mut tokens = TokenStream2::new();
        for env_var in env_vars {
            tokens.append_all(quote!(#env_var,))
        }

        if let Some(default) = default_map.get(&field) {
            fields.append_all(quote! {
                #field: parse_value(&[#tokens])
                    .unwrap_or_else(|| std::str::FromStr::from_str(#default).unwrap()),
            });
        } else {
            fields.append_all(quote! {
                #field: parse_value(&[#tokens]),
            });
        }
    }

    let mut methods = TokenStream2::new();
    for (field, env_vars) in field_map {
        let mut tokens = TokenStream2::new();
        for env_var in env_vars {
            tokens.append_all(quote!(#env_var,))
        }

        let method_name = Ident::new(&format!("{}_vars", field), Span::call_site());
        methods.append_all(quote! {
            pub fn #method_name() -> Vec<std::string::String> {
                [#tokens]
                    .iter()
                    .map(|s| s.to_string())
                    .collect()
            }
        });
    }

    return quote! {

        impl #name {
            pub fn parse() -> Self {
                Self {
                    #fields
                }
            }

            #methods

        }

        fn first_non_empty(envs: &[&str]) -> Option<String> {
            for env in envs {
                if let Ok(v) = std::env::var(env) {
                    return Some(v);
                }
            }
            return None;
        }

        fn parse_value<T: std::str::FromStr>(envs: &[&str]) -> Option<T> {
            first_non_empty(envs).and_then(|s| T::from_str(&s).ok())
        }

    };
}

fn generate_tests(
    name: &Ident,
    field_map: &HashMap<Ident, Vec<String>>,
    default_map: &HashMap<Ident, Lit>,
    example_map: &HashMap<Ident, Lit>,
) -> TokenStream2 {
    let mut tests = TokenStream2::new();

    for (field, env_vars) in field_map {
        let mut tokens = TokenStream2::new();
        for env_var in env_vars {
            tokens.append_all(quote!(#env_var,))
        }

        if let Some(test_data) = example_map.get(&field) {
            if let Some(_) = default_map.get(&field) {
                tests.append_all(quote! {
                    for env in &[#tokens] {
                        // reset env vars
                        for env in &[#tokens] {
                            std::env::remove_var(env);
                        }
                        std::env::set_var(env, #test_data);
                        assert_eq!(Some(#name::parse().#field), std::str::FromStr::from_str(#test_data).ok());
                    }
                });
            } else {
                tests.append_all(quote! {
                    for env in &[#tokens] {
                        // reset env vars
                        for env in &[#tokens] {
                            std::env::remove_var(env);
                        }
                        std::env::set_var(env, #test_data);
                        assert_eq!(#name::parse().#field, std::str::FromStr::from_str(#test_data).ok());
                    }
                });
            }
        }
    }

    return quote! {

        #[cfg(test)]
        mod env_config_tests {
            use super::#name;

            #[test]
            fn env_config_test() {
                #tests
            }
        }

    };
}