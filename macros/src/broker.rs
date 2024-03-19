use inflector::Inflector;
use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::{
    parenthesized,
    parse::{Parse, ParseStream, Parser},
    parse_macro_input, Attribute, Data, DataStruct, DeriveInput, Field, Fields, FieldsNamed, Ident,
    Token,
};

#[allow(clippy::too_many_lines)]
pub fn proc_macro_derive(input: TokenStream) -> TokenStream {
    let DeriveInput { ident, data, .. } = parse_macro_input!(input);
    let Data::Struct(DataStruct { fields, .. }) = data else { panic!("must be struct") };
    let Fields::Named(FieldsNamed { named: fields, .. }) = fields else {
        panic!("must have named fields")
    };

    let agent_fields = fields.iter().filter_map(|field| {
        field
            .attrs
            .iter()
            .find_map(|Attribute { path, tokens, .. }| {
                path.segments
                    .first()
                    .and_then(|segment| (segment.ident == "agent").then_some(tokens))
            })
            .map(|tokens| {
                let parser = |tokens: ParseStream| {
                    let content;
                    parenthesized!(content in tokens);
                    content.parse_terminated::<_, Token![,]>(Ident::parse)
                };
                let tags =
                    parser.parse(tokens.clone().into()).expect("failed to parse `agent` attr");
                (tags, field)
            })
    });

    let constructor_name = format_ident!("new_{}", ident.to_string().to_snake_case());
    let constructor_fields = agent_fields
        .clone()
        .map(|(_, Field { ident, .. })| quote!(#ident: crate::brokers::AgentCell::Vacant));
    let constructor = quote! {
        macro_rules! #constructor_name {
            ($($tokens:tt)*) => {
                #ident {
                    #(#constructor_fields,)*
                    $($tokens)*
                }
            };
        }
    };

    let run_fut_name = format_ident!("Run{}", ident);
    let run_handlers = agent_fields.clone().map(|(_, field)| {
        let ident = field.ident.as_ref().unwrap();
        let handler = format_ident!("handle_{}", ident);
        quote! {
            if let Some(port) = fut.broker.#ident.enabled() {
                loop {
                    match ::futures::StreamExt::poll_next_unpin(port, cx) {
                        ::std::task::Poll::Ready(Some(output)) if output.source_ts > fence => {
                            match fut.broker.#handler(fut.plan, output) {
                                ::std::result::Result::Ok(crate::brokers::BrokerFlow::Break) => {
                                    return ::std::task::Poll::Ready(::std::result::Result::Ok(()));
                                }
                                ::std::result::Result::Ok(crate::brokers::BrokerFlow::Continue) => {
                                    continue 'outer;
                                }
                                ::std::result::Result::Err(err) => {
                                    return ::std::task::Poll::Ready(
                                        ::std::result::Result::Err(err),
                                    );
                                }
                            }
                        }
                        ::std::task::Poll::Ready(::std::option::Option::Some(_)) => {
                            continue;
                        }
                        ::std::task::Poll::Ready(::std::option::Option::None) => {
                            return ::std::task::Poll::Ready(
                                ::std::result::Result::Err(
                                    ::eyre::eyre!("agent {} exited", ::std::stringify!(#ident)),
                                ),
                            );
                        }
                        ::std::task::Poll::Pending => {
                            break;
                        }
                    }
                }
            }
        }
    });
    let run = quote! {
        #[allow(missing_docs)]
        pub struct #run_fut_name<'a> {
            broker: &'a mut #ident,
            plan: &'a mut dyn Plan,
            fence: ::std::time::Instant,
        }

        impl ::futures::future::Future for #run_fut_name<'_> {
            type Output = ::eyre::Result<()>;

            fn poll(
                mut self: ::std::pin::Pin<&mut Self>,
                cx: &mut ::std::task::Context<'_>,
            ) -> ::std::task::Poll<Self::Output> {
                let fence = self.fence;
                let fut = self.as_mut().get_mut();
                'outer: loop {
                    #(#run_handlers)*
                    match fut.broker.poll_extra(fut.plan, cx, fence) {
                        ::std::result::Result::Ok(::std::option::Option::Some(poll)) => {
                            break poll.map(Ok);
                        }
                        ::std::result::Result::Ok(::std::option::Option::None) => {
                            continue;
                        }
                        ::std::result::Result::Err(err) => {
                            return ::std::task::Poll::Ready(
                                ::std::result::Result::Err(err),
                            );
                        }
                    }
                }
            }
        }

        impl #ident {
            #[allow(missing_docs)]
            pub fn run<'a>(&'a mut self, plan: &'a mut dyn Plan) -> #run_fut_name<'a> {
                Self::run_with_fence(self, plan, ::std::time::Instant::now())
            }

            #[allow(missing_docs)]
            pub fn run_with_fence<'a>(
                &'a mut self,
                plan: &'a mut dyn Plan,
                fence: ::std::time::Instant,
            ) -> #run_fut_name<'a> {
                #run_fut_name {
                    broker: self,
                    plan,
                    fence,
                }
            }
        }
    };

    let methods = agent_fields.clone().map(|(tags, field)| {
        let ident = field.ident.as_ref().unwrap();
        let enable = format_ident!("enable_{}", ident);
        let try_enable = format_ident!("try_enable_{}", ident);
        let disable = format_ident!("disable_{}", ident);
        let async_ = tags.iter().any(|tag| tag == "async").then(|| quote!(async));
        let constructor = if tags.iter().any(|tag| tag == "default") {
            quote!(Default::default())
        } else {
            let constructor = format_ident!("init_{}", ident);
            if async_.is_some() {
                quote!(self.#constructor().await?)
            } else {
                quote!(self.#constructor())
            }
        };
        let constructor = if tags.iter().any(|tag| tag == "process") {
            quote!(crate::agents::AgentProcess::spawn_process(#constructor))
        } else if tags.iter().any(|tag| tag == "thread") {
            quote!(crate::agents::AgentThread::spawn_thread(#constructor)?)
        } else if tags.iter().any(|tag| tag == "task") {
            quote!(crate::agents::AgentTask::spawn_task(#constructor))
        } else {
            panic!("must have `task`, `thread`, or `process` tag");
        };

        quote! {
            #[allow(missing_docs)]
            pub #async_ fn #enable(&mut self) -> eyre::Result<()> {
                match ::std::mem::replace(&mut self.#ident, crate::brokers::AgentCell::Vacant) {
                    crate::brokers::AgentCell::Vacant => {
                        self.#ident = crate::brokers::AgentCell::Enabled(#constructor);
                    }
                    crate::brokers::AgentCell::Enabled(agent)
                    | crate::brokers::AgentCell::Disabled(agent) => {
                        self.#ident = crate::brokers::AgentCell::Enabled(agent);
                    }
                }
                Ok(())
            }

            #[allow(missing_docs)]
            pub fn #try_enable(&mut self) {
                match ::std::mem::replace(&mut self.#ident, crate::brokers::AgentCell::Vacant) {
                    crate::brokers::AgentCell::Vacant => {}
                    crate::brokers::AgentCell::Enabled(agent)
                    | crate::brokers::AgentCell::Disabled(agent) => {
                        self.#ident = crate::brokers::AgentCell::Enabled(agent);
                    }
                }
            }

            #[allow(missing_docs)]
            pub fn #disable(&mut self) {
                match ::std::mem::replace(&mut self.#ident, crate::brokers::AgentCell::Vacant) {
                    crate::brokers::AgentCell::Vacant => {}
                    crate::brokers::AgentCell::Enabled(agent)
                    | crate::brokers::AgentCell::Disabled(agent) => {
                        self.#ident = crate::brokers::AgentCell::Disabled(agent);
                    }
                }
            }
        }
    });

    let disable_agents = agent_fields.map(|(_, field)| {
        let disable = format_ident!("disable_{}", field.ident.as_ref().unwrap());
        quote!(#disable)
    });

    let expanded = quote! {
        #constructor
        #run

        impl #ident {
            #(#methods)*

            #[allow(missing_docs)]
            pub fn disable_agents(&mut self) {
                #(self.#disable_agents();)*
            }
        }
    };
    expanded.into()
}
