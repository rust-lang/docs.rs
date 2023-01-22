pub(super) trait MetricFromOpts: Sized {
    fn from_opts(opts: prometheus::Opts) -> Result<Self, prometheus::Error>;
}

#[macro_export]
macro_rules! metrics {
    (
        $vis:vis struct $name:ident {
            $(
                #[doc = $help:expr]
                $(#[$meta:meta])*
                $metric_vis:vis $metric:ident: $ty:ty $([$($label:expr),* $(,)?])?
            ),* $(,)?
        }
        namespace: $namespace:expr,
    ) => {
        $vis struct $name {
            registry: prometheus::Registry,
            $(
                $(#[$meta])*
                $metric_vis $metric: $ty,
            )*
            pub(crate) recently_accessed_releases: RecentlyAccessedReleases,
            pub(crate) cdn_invalidation_time: prometheus::HistogramVec,
            pub(crate) cdn_queue_time: prometheus::HistogramVec,
        }
        impl $name {
            $vis fn new() -> Result<Self, prometheus::Error> {
                let registry = prometheus::Registry::new();
                $(
                    $(#[$meta])*
                    let $metric = <$ty>::from_opts(
                        prometheus::Opts::new(stringify!($metric), $help)
                            .namespace($namespace)
                            $(.variable_labels(vec![$($label.into()),*]))?
                    )?;
                    $(#[$meta])*
                    registry.register(Box::new($metric.clone()))?;
                )*
                Ok(Self {
                    registry,
                    recently_accessed_releases: RecentlyAccessedReleases::new(),
                    cdn_invalidation_time: prometheus::HistogramVec::new(
                        prometheus::HistogramOpts::new(
                            "cdn_invalidation_time",
                            "duration of CDN invalidations after having beeing sent to the CDN.",
                        )
                        .buckets($crate::metrics::CDN_INVALIDATION_HISTOGRAM_BUCKETS.to_vec())
                        .variable_label("distribution"),
                        &["distribution"],
                    )?,
                    cdn_queue_time: prometheus::HistogramVec::new(
                        prometheus::HistogramOpts::new(
                            "cdn_queue_time",
                            "queue time of CDN invalidations before they are sent to the CDN. ",
                        )
                        .buckets($crate::metrics::CDN_INVALIDATION_HISTOGRAM_BUCKETS.to_vec())
                        .variable_label("distribution"),
                        &["distribution"],
                    )?,
                    $(
                        $(#[$meta])*
                        $metric,
                    )*
                })
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
            fn from_opts(opts: prometheus::Opts) -> Result<Self, prometheus::Error> {
                $name::with_opts(opts)
            }
        }
    };
    ($name:ident as vec) => {
        use prometheus::$name;
        impl MetricFromOpts for $name {
            fn from_opts(opts: prometheus::Opts) -> Result<Self, prometheus::Error> {
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
