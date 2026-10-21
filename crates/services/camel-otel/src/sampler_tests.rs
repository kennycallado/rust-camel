use super::OtelService;
use crate::config::{OtelConfig, OtelSampler};
use opentelemetry::trace::{
    Link, Span, SpanContext, SpanId, SpanKind, TraceContextExt, TraceFlags, TraceId, TraceState,
    Tracer, TracerProvider,
};
use opentelemetry::{Context, KeyValue};
use opentelemetry_sdk::trace::{
    InMemorySpanExporter, Sampler, SamplingDecision, SdkTracerProvider, ShouldSample,
};

#[test]
fn sampler_always_on_wraps_parent_based() {
    let config =
        OtelConfig::new("http://localhost:4317", "svc").with_sampler(OtelSampler::AlwaysOn);
    let result = OtelService::to_sdk_sampler(&config).expect("valid sampler config");
    assert!(matches!(result, Sampler::ParentBased(_)));
}

#[test]
fn sampler_ratio_based_wraps_parent_based() {
    let config = OtelConfig::new("http://localhost:4317", "svc")
        .with_sampler(OtelSampler::TraceIdRatioBased(0.5));
    let result = OtelService::to_sdk_sampler(&config).expect("valid sampler config");
    assert!(matches!(result, Sampler::ParentBased(_)));
}

#[test]
fn sampler_always_off_wraps_parent_based() {
    let config =
        OtelConfig::new("http://localhost:4317", "svc").with_sampler(OtelSampler::AlwaysOff);
    let result = OtelService::to_sdk_sampler(&config).expect("valid sampler config");
    assert!(matches!(result, Sampler::ParentBased(_)));
}

#[test]
fn sampler_unsampled_parent_drops_child() {
    let config = OtelConfig::new("http://localhost:4317", "svc")
        .with_sampler(OtelSampler::TraceIdRatioBased(1.0));
    let sampler = OtelService::to_sdk_sampler(&config).expect("valid sampler config");
    let parent = SpanContext::new(
        TraceId::from_hex("12345678901234567890123456789012").expect("valid trace id"),
        SpanId::from_hex("1234567890123456").expect("valid span id"),
        TraceFlags::default(),
        true,
        TraceState::default(),
    );
    let cx = Context::new().with_remote_span_context(parent);
    let result = sampler.should_sample(
        Some(&cx),
        TraceId::from_hex("abcdefabcdefabcdefabcdefabcdefab").expect("valid trace id"),
        "child",
        &SpanKind::Internal,
        &[],
        &[],
    );
    assert!(matches!(result.decision, SamplingDecision::Drop));
}

#[test]
fn sampler_sampled_parent_records_child_regardless_of_ratio() {
    let config = OtelConfig::new("http://localhost:4317", "svc")
        .with_sampler(OtelSampler::TraceIdRatioBased(0.0));
    let sampler = OtelService::to_sdk_sampler(&config).expect("valid sampler config");
    let parent = SpanContext::new(
        TraceId::from_hex("12345678901234567890123456789012").expect("valid trace id"),
        SpanId::from_hex("1234567890123456").expect("valid span id"),
        TraceFlags::SAMPLED,
        true,
        TraceState::default(),
    );
    let cx = Context::new().with_remote_span_context(parent);
    let result = sampler.should_sample(
        Some(&cx),
        TraceId::from_hex("abcdefabcdefabcdefabcdefabcdefab").expect("valid trace id"),
        "child",
        &SpanKind::Internal,
        &[],
        &[],
    );
    assert!(matches!(result.decision, SamplingDecision::RecordAndSample));
}

#[test]
fn sampler_root_ratio_delegate_drops_at_zero() {
    let config = OtelConfig::new("http://localhost:4317", "svc")
        .with_sampler(OtelSampler::TraceIdRatioBased(0.0));
    let sampler = OtelService::to_sdk_sampler(&config).expect("valid sampler config");
    let result = sampler.should_sample(
        None,
        TraceId::from_hex("abcdefabcdefabcdefabcdefabcdefab").expect("valid trace id"),
        "root",
        &SpanKind::Internal,
        &[],
        &[],
    );
    assert!(matches!(result.decision, SamplingDecision::Drop));
}

/// Hand-built link target with fixed valid ids and a caller-chosen
/// sampled flag.
fn link_target(sampled: bool) -> SpanContext {
    SpanContext::new(
        TraceId::from_hex("12345678901234567890123456789012").expect("valid trace id"),
        SpanId::from_hex("1234567890123456").expect("valid span id"),
        if sampled {
            TraceFlags::SAMPLED
        } else {
            TraceFlags::default()
        },
        true,
        TraceState::default(),
    )
}

fn sample_with_link(sampler: &dyn ShouldSample, link: SpanContext) -> SamplingDecision {
    sampler
        .should_sample(
            None,
            TraceId::from_hex("abcdefabcdefabcdefabcdefabcdefab").expect("valid trace id"),
            "item",
            &SpanKind::Internal,
            &[],
            &[Link::new(link, Vec::new(), 0)],
        )
        .decision
}

#[test]
fn link_aware_root_follows_sampled_link() {
    let config =
        OtelConfig::new("http://localhost:4317", "svc").with_sampler(OtelSampler::AlwaysOff);
    let sampler = OtelService::to_sdk_sampler(&config).expect("valid sampler config");
    let decision = sample_with_link(&sampler, link_target(true));
    assert!(matches!(decision, SamplingDecision::RecordAndSample));
}

#[test]
fn link_aware_root_follows_unsampled_link() {
    let config =
        OtelConfig::new("http://localhost:4317", "svc").with_sampler(OtelSampler::AlwaysOn);
    let sampler = OtelService::to_sdk_sampler(&config).expect("valid sampler config");
    let decision = sample_with_link(&sampler, link_target(false));
    assert!(matches!(decision, SamplingDecision::Drop));
}

#[test]
fn link_aware_root_without_links_delegates_inner() {
    let off_config =
        OtelConfig::new("http://localhost:4317", "svc").with_sampler(OtelSampler::AlwaysOff);
    let on_config =
        OtelConfig::new("http://localhost:4317", "svc").with_sampler(OtelSampler::AlwaysOn);
    let off = OtelService::to_sdk_sampler(&off_config).expect("valid sampler config");
    let on = OtelService::to_sdk_sampler(&on_config).expect("valid sampler config");
    assert!(matches!(
        off.should_sample(
            None,
            TraceId::from_hex("abcdefabcdefabcdefabcdefabcdefab").expect("valid trace id"),
            "root",
            &SpanKind::Internal,
            &[],
            &[],
        )
        .decision,
        SamplingDecision::Drop
    ));
    assert!(matches!(
        on.should_sample(
            None,
            TraceId::from_hex("abcdefabcdefabcdefabcdefabcdefab").expect("valid trace id"),
            "root",
            &SpanKind::Internal,
            &[],
            &[],
        )
        .decision,
        SamplingDecision::RecordAndSample
    ));
}

#[test]
fn provider_mints_linked_root_with_parent_flag() {
    for link_sampled in [true, false] {
        let exporter = InMemorySpanExporter::default();
        let config =
            OtelConfig::new("http://localhost:4317", "svc").with_sampler(OtelSampler::AlwaysOff);
        let sampler = OtelService::to_sdk_sampler(&config).expect("valid sampler config");
        let provider = SdkTracerProvider::builder()
            .with_sampler(sampler)
            .with_simple_exporter(exporter.clone())
            .build();
        let tracer = provider.tracer("splittrace-item");
        let mut span = tracer.build_with_context(
            tracer.span_builder("item").with_links(vec![Link::new(
                link_target(link_sampled),
                Vec::new(),
                0,
            )]),
            &Context::new(),
        );
        span.end();
        provider.force_flush().expect("flush item span");
        let exported = exporter.get_finished_spans().expect("exported spans");
        if link_sampled {
            assert_eq!(exported.len(), 1, "sampled link must export the item span");
            assert!(
                exported[0].span_context.is_sampled(),
                "exported item span must carry the sampled flag"
            );
        } else {
            assert!(
                exported.is_empty(),
                "unsampled link must drop the item span before export"
            );
        }
        provider.shutdown().expect("provider shutdown");
    }
}

#[test]
fn to_sdk_sampler_wraps_root_link_aware() {
    let unsampled = link_target(false);
    for sampler in [
        OtelSampler::AlwaysOn,
        OtelSampler::AlwaysOff,
        OtelSampler::TraceIdRatioBased(1.0),
    ] {
        let config = OtelConfig::new("http://localhost:4317", "svc").with_sampler(sampler.clone());
        let Sampler::ParentBased(delegate) =
            OtelService::to_sdk_sampler(&config).expect("valid sampler config")
        else {
            panic!("to_sdk_sampler must wrap {sampler:?} in ParentBased");
        };
        let decision = sample_with_link(delegate.as_ref(), unsampled.clone());
        assert!(
            matches!(decision, SamplingDecision::Drop),
            "unsampled link must drop the root span even under {sampler:?}"
        );
    }
}

#[test]
fn exclude_routes_drops_matching_root() {
    let config = OtelConfig::new("http://localhost:4317", "svc")
        .with_sampler(OtelSampler::AlwaysOn)
        .with_exclude_routes(vec!["skip-request*".into()]);
    let sampler = OtelService::to_sdk_sampler(&config).expect("valid sampler config");
    let result = sampler.should_sample(
        None,
        TraceId::from_hex("abcdefabcdefabcdefabcdefabcdefab").expect("valid trace id"),
        "skip-request",
        &SpanKind::Internal,
        &[KeyValue::new("route_id", "skip-request")],
        &[],
    );
    assert!(matches!(result.decision, SamplingDecision::Drop));
}

#[test]
fn exclude_routes_keeps_other_roots() {
    let config = OtelConfig::new("http://localhost:4317", "svc")
        .with_sampler(OtelSampler::AlwaysOn)
        .with_exclude_routes(vec!["skip-request*".into()]);
    let sampler = OtelService::to_sdk_sampler(&config).expect("valid sampler config");
    let result = sampler.should_sample(
        None,
        TraceId::from_hex("abcdefabcdefabcdefabcdefabcdefab").expect("valid trace id"),
        "orders-in",
        &SpanKind::Internal,
        &[KeyValue::new("route_id", "orders-in")],
        &[],
    );
    assert!(matches!(result.decision, SamplingDecision::RecordAndSample));
}

#[test]
fn sampled_parent_wins_over_exclusion() {
    let config = OtelConfig::new("http://localhost:4317", "svc")
        .with_sampler(OtelSampler::AlwaysOn)
        .with_exclude_routes(vec!["skip-*".into()]);
    let sampler = OtelService::to_sdk_sampler(&config).expect("valid sampler config");
    let parent = SpanContext::new(
        TraceId::from_hex("12345678901234567890123456789012").expect("valid trace id"),
        SpanId::from_hex("1234567890123456").expect("valid span id"),
        TraceFlags::SAMPLED,
        true,
        TraceState::default(),
    );
    let cx = Context::new().with_remote_span_context(parent);
    let result = sampler.should_sample(
        Some(&cx),
        TraceId::from_hex("abcdefabcdefabcdefabcdefabcdefab").expect("valid trace id"),
        "child",
        &SpanKind::Internal,
        &[KeyValue::new("route_id", "skip-me")],
        &[],
    );
    assert!(matches!(result.decision, SamplingDecision::RecordAndSample));
}

#[test]
fn linked_split_item_follows_origin() {
    let config = OtelConfig::new("http://localhost:4317", "svc")
        .with_sampler(OtelSampler::AlwaysOn)
        .with_exclude_routes(vec!["skip-*".into()]);
    let sampler = OtelService::to_sdk_sampler(&config).expect("valid sampler config");
    let result = sampler.should_sample(
        None,
        TraceId::from_hex("abcdefabcdefabcdefabcdefabcdefab").expect("valid trace id"),
        "item",
        &SpanKind::Internal,
        &[KeyValue::new("route_id", "skip-me")],
        &[Link::new(link_target(false), Vec::new(), 0)],
    );
    assert!(matches!(result.decision, SamplingDecision::Drop));
}

#[test]
fn sampled_link_wins_over_exclusion() {
    let config = OtelConfig::new("http://localhost:4317", "svc")
        .with_sampler(OtelSampler::AlwaysOn)
        .with_exclude_routes(vec!["skip-*".into()]);
    let sampler = OtelService::to_sdk_sampler(&config).expect("valid sampler config");
    let result = sampler.should_sample(
        None,
        TraceId::from_hex("abcdefabcdefabcdefabcdefabcdefab").expect("valid trace id"),
        "item",
        &SpanKind::Internal,
        &[KeyValue::new("route_id", "skip-me")],
        &[Link::new(link_target(true), Vec::new(), 0)],
    );
    assert!(matches!(result.decision, SamplingDecision::RecordAndSample));
}

#[test]
fn exclude_routes_empty_list_delegates_to_inner() {
    let config =
        OtelConfig::new("http://localhost:4317", "svc").with_sampler(OtelSampler::AlwaysOn);
    let sampler = OtelService::to_sdk_sampler(&config).expect("valid sampler config");
    let result = sampler.should_sample(
        None,
        TraceId::from_hex("abcdefabcdefabcdefabcdefabcdefab").expect("valid trace id"),
        "root",
        &SpanKind::Internal,
        &[KeyValue::new("route_id", "any-route")],
        &[],
    );
    assert!(matches!(result.decision, SamplingDecision::RecordAndSample));
}
