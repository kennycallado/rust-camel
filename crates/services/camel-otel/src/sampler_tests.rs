use super::OtelService;
use crate::config::OtelSampler;
use opentelemetry::Context;
use opentelemetry::trace::{
    SpanContext, SpanId, SpanKind, TraceContextExt, TraceFlags, TraceId, TraceState,
};
use opentelemetry_sdk::trace::{Sampler, SamplingDecision, ShouldSample};

#[test]
fn sampler_always_on_wraps_parent_based() {
    let result = OtelService::to_sdk_sampler(&OtelSampler::AlwaysOn);
    assert!(matches!(result, Sampler::ParentBased(_)));
}

#[test]
fn sampler_ratio_based_wraps_parent_based() {
    let result = OtelService::to_sdk_sampler(&OtelSampler::TraceIdRatioBased(0.5));
    assert!(matches!(result, Sampler::ParentBased(_)));
}

#[test]
fn sampler_always_off_wraps_parent_based() {
    let result = OtelService::to_sdk_sampler(&OtelSampler::AlwaysOff);
    assert!(matches!(result, Sampler::ParentBased(_)));
}

#[test]
fn sampler_unsampled_parent_drops_child() {
    let sampler = OtelService::to_sdk_sampler(&OtelSampler::TraceIdRatioBased(1.0));
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
    let sampler = OtelService::to_sdk_sampler(&OtelSampler::TraceIdRatioBased(0.0));
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
    let sampler = OtelService::to_sdk_sampler(&OtelSampler::TraceIdRatioBased(0.0));
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
