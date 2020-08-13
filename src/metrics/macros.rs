pub(super) trait MetricFromOpts: Sized {
    fn from_opts(opts: prometheus::Opts) -> Result<Self, prometheus::Error>;
}

#[macro_export]
macro_rules! metrics {
    (
        $vis:vis struct $name:ident {
            $(#[doc = $help:expr] $metric:ident: $ty:ty $([$($label:expr),*])?,)*
        }
        metrics visibility: $metric_vis:vis,
        namespace: $namespace:expr,
    ) => {
        $vis struct $name {
            registry: Registry,
            $($metric_vis $metric: $ty,)*
        }
        impl $name {
            $vis fn new() -> Result<Self, Error> {
                let registry = Registry::new();
                $(
                    let $metric = <$ty>::from_opts(
                        Opts::new(stringify!($metric), $help)
                            .namespace($namespace)
                            $(.variable_labels(vec![$($label.into()),*]))?
                    )?;
                    registry.register(Box::new($metric.clone()))?;
                )*
                Ok(Self { registry, $($metric,)* })
            }
        }
        impl std::fmt::Debug for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                write!(f, "{}", stringify!($name))
            }
        }
    };
}

#[macro_export]
macro_rules! load_metric_type {
    ($name:ident as single) => {
        use prometheus::$name;
        impl MetricFromOpts for $name {
            fn from_opts(opts: Opts) -> Result<Self, prometheus::Error> {
                $name::with_opts(opts)
            }
        }
    };
    ($name:ident as vec) => {
        use prometheus::$name;
        impl MetricFromOpts for $name {
            fn from_opts(opts: Opts) -> Result<Self, prometheus::Error> {
                $name::new(
                    opts.clone().into(),
                    opts.variable_labels
                        .iter()
                        .map(|s| s.as_str())
                        .collect::<Vec<_>>()
                        .as_slice(),
                )
            }
        }
    };
}
