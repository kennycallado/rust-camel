use super::{
    get_bool_header, get_f64_header, get_i64_header, get_str_header, get_str_vec_header,
    require_key, require_str_header,
};
use crate::config::RedisCommand;
use camel_component_api::{Body, CamelError, Exchange};
use redis::aio::MultiplexedConnection;

pub(crate) fn is_geo_command(cmd: &RedisCommand) -> bool {
    matches!(
        cmd,
        RedisCommand::Geoadd
            | RedisCommand::Geopos
            | RedisCommand::Geodist
            | RedisCommand::Geosearch
            | RedisCommand::Geohash
    )
}

fn validate_longitude(v: f64) -> Result<(), CamelError> {
    if !v.is_finite() || !(-180.0..=180.0).contains(&v) {
        return Err(CamelError::ProcessorError(format!(
            "Invalid CamelRedis.Longitude: {v} must be a finite number in [-180.0, 180.0]"
        )));
    }
    Ok(())
}

fn validate_latitude(v: f64) -> Result<(), CamelError> {
    if !v.is_finite() || !(-90.0..=90.0).contains(&v) {
        return Err(CamelError::ProcessorError(format!(
            "Invalid CamelRedis.Latitude: {v} must be a finite number in [-90.0, 90.0]"
        )));
    }
    Ok(())
}

fn require_f64_header(exchange: &Exchange, name: &str) -> Result<f64, CamelError> {
    get_f64_header(exchange, name)?
        .ok_or_else(|| CamelError::ProcessorError(format!("Missing required header: {name}")))
}

fn validate_positive(v: f64, header: &'static str) -> Result<(), CamelError> {
    if !v.is_finite() || v <= 0.0 {
        return Err(CamelError::ProcessorError(format!(
            "Invalid {header}: {v} must be a finite number greater than 0"
        )));
    }
    Ok(())
}

#[derive(Debug)]
enum GeoSearchShape {
    Radius(f64),
    Box { width: f64, height: f64 },
}

fn numeric_shape_header(exchange: &Exchange, name: &str) -> Result<Option<f64>, CamelError> {
    get_f64_header(exchange, name)
}

fn resolve_geo_search_shape(exchange: &Exchange) -> Result<GeoSearchShape, CamelError> {
    let radius = numeric_shape_header(exchange, "CamelRedis.Radius")?;
    let width = numeric_shape_header(exchange, "CamelRedis.Width")?;
    let height = numeric_shape_header(exchange, "CamelRedis.Height")?;
    if let Some(radius) = radius {
        if width.is_some() || height.is_some() {
            return Err(CamelError::ProcessorError(
                "CamelRedis.Radius and CamelRedis.Width/CamelRedis.Height are mutually exclusive: use either a radius or a box"
                    .into(),
            ));
        }
        validate_positive(radius, "CamelRedis.Radius")?;
        return Ok(GeoSearchShape::Radius(radius));
    }
    match (width, height) {
        (Some(width), Some(height)) => {
            validate_positive(width, "CamelRedis.Width")?;
            validate_positive(height, "CamelRedis.Height")?;
            Ok(GeoSearchShape::Box { width, height })
        }
        (Some(_), None) => Err(CamelError::ProcessorError(
            "CamelRedis.Height is required when CamelRedis.Width is set".into(),
        )),
        (None, Some(_)) => Err(CamelError::ProcessorError(
            "CamelRedis.Width is required when CamelRedis.Height is set".into(),
        )),
        (None, None) => Err(CamelError::ProcessorError(
            "GEOSEARCH requires a radius or a box: set CamelRedis.Radius, or CamelRedis.Width and CamelRedis.Height"
                .into(),
        )),
    }
}

fn require_member(exchange: &Exchange) -> Result<String, CamelError> {
    require_str_header(exchange, "CamelRedis.Member").map(|s| s.to_string())
}

fn require_member2(exchange: &Exchange) -> Result<String, CamelError> {
    require_str_header(exchange, "CamelRedis.Member2").map(|s| s.to_string())
}

fn require_members(exchange: &Exchange) -> Result<Vec<String>, CamelError> {
    get_str_vec_header(exchange, "CamelRedis.Members")
        .filter(|members| !members.is_empty())
        .ok_or_else(|| {
            CamelError::ProcessorError(
                "Missing or empty required header: CamelRedis.Members".into(),
            )
        })
}

fn resolve_geoadd_args(exchange: &Exchange) -> Result<(String, f64, f64, String), CamelError> {
    let key = require_key(exchange)?;
    let longitude = require_f64_header(exchange, "CamelRedis.Longitude")?;
    validate_longitude(longitude)?;
    let latitude = require_f64_header(exchange, "CamelRedis.Latitude")?;
    validate_latitude(latitude)?;
    let member = require_member(exchange)?;
    Ok((key, longitude, latitude, member))
}

fn resolve_geo_unit(exchange: &Exchange) -> Result<&'static str, CamelError> {
    let Some(unit) = get_str_header(exchange, "CamelRedis.Unit") else {
        return Ok("m");
    };
    match unit.to_ascii_lowercase().as_str() {
        "m" => Ok("m"),
        "km" => Ok("km"),
        "mi" => Ok("mi"),
        "ft" => Ok("ft"),
        _ => Err(CamelError::ProcessorError(format!(
            "Invalid CamelRedis.Unit: {unit} must be one of m, km, mi, ft"
        ))),
    }
}

fn json_from_geopos(positions: &[Option<(f64, f64)>]) -> serde_json::Value {
    serde_json::Value::Array(
        positions
            .iter()
            .map(|position| match position {
                Some((lon, lat)) => serde_json::json!([lon, lat]),
                None => serde_json::Value::Null,
            })
            .collect(),
    )
}

async fn execute_geoadd(
    exchange: &mut Exchange,
    conn: &mut MultiplexedConnection,
) -> Result<(), CamelError> {
    let (key, longitude, latitude, member) = resolve_geoadd_args(exchange)?;
    let n: i64 = redis::cmd("GEOADD")
        .arg(&key)
        .arg(longitude)
        .arg(latitude)
        .arg(member)
        .query_async(conn)
        .await
        .map_err(|e| crate::transport_error::redis_error_to_camel("GEOADD", e))?;
    exchange.input.body = Body::Json(serde_json::json!(n));
    Ok(())
}

async fn execute_geopos(
    exchange: &mut Exchange,
    conn: &mut MultiplexedConnection,
) -> Result<(), CamelError> {
    let key = require_key(exchange)?;
    let members = require_members(exchange)?;
    let positions: Vec<Option<(f64, f64)>> = redis::cmd("GEOPOS")
        .arg(&key)
        .arg(&members[..])
        .query_async(conn)
        .await
        .map_err(|e| crate::transport_error::redis_error_to_camel("GEOPOS", e))?;
    exchange.input.body = Body::Json(json_from_geopos(&positions));
    Ok(())
}

fn json_from_geohashes(hashes: Vec<Option<String>>) -> serde_json::Value {
    serde_json::json!(hashes)
}

fn build_geodist_cmd(key: &str, m1: &str, m2: &str, unit: &str) -> redis::Cmd {
    let mut cmd = redis::cmd("GEODIST");
    cmd.arg(key).arg(m1).arg(m2).arg(unit);
    cmd
}

async fn execute_geodist(
    exchange: &mut Exchange,
    conn: &mut MultiplexedConnection,
) -> Result<(), CamelError> {
    let key = require_key(exchange)?;
    let m1 = require_member(exchange)?;
    let m2 = require_member2(exchange)?;
    let unit = resolve_geo_unit(exchange)?;
    let distance: Option<f64> = build_geodist_cmd(&key, &m1, &m2, unit)
        .query_async(conn)
        .await
        .map_err(|e| crate::transport_error::redis_error_to_camel("GEODIST", e))?;
    exchange.input.body = Body::Json(serde_json::json!(distance));
    Ok(())
}

async fn execute_geohash(
    exchange: &mut Exchange,
    conn: &mut MultiplexedConnection,
) -> Result<(), CamelError> {
    let key = require_key(exchange)?;
    let members = require_members(exchange)?;
    let hashes: Vec<Option<String>> = redis::cmd("GEOHASH")
        .arg(&key)
        .arg(&members[..])
        .query_async(conn)
        .await
        .map_err(|e| crate::transport_error::redis_error_to_camel("GEOHASH", e))?;
    exchange.input.body = Body::Json(json_from_geohashes(hashes));
    Ok(())
}

fn value_to_f64(value: &redis::Value) -> Option<f64> {
    match value {
        redis::Value::BulkString(bytes) => std::str::from_utf8(bytes).ok()?.parse().ok(),
        redis::Value::Int(i) => Some(*i as f64),
        redis::Value::Double(d) => Some(*d),
        _ => None,
    }
}

fn value_into_member(value: redis::Value) -> Option<String> {
    match value {
        redis::Value::BulkString(bytes) => String::from_utf8(bytes).ok(),
        redis::Value::SimpleString(text) => Some(text),
        _ => None,
    }
}

fn round_to_6(x: f64) -> f64 {
    (x * 1e6).round() / 1e6
}

fn parse_coord_pair(value: redis::Value) -> Result<(f64, f64), CamelError> {
    let values = match value {
        redis::Value::Array(items) => items,
        _ => {
            return Err(CamelError::ProcessorError(
                "GEOSEARCH reply row coordinate pair is not a list".into(),
            ));
        }
    };
    if values.len() != 2 {
        return Err(CamelError::ProcessorError(format!(
            "GEOSEARCH reply row coordinate pair must have 2 elements, got {}",
            values.len()
        )));
    }
    let lon = value_to_f64(&values[0]).ok_or_else(|| {
        CamelError::ProcessorError("GEOSEARCH reply row longitude is not numeric".into())
    })?;
    let lat = value_to_f64(&values[1]).ok_or_else(|| {
        CamelError::ProcessorError("GEOSEARCH reply row latitude is not numeric".into())
    })?;
    Ok((lon, lat))
}

fn json_from_geo_rows(
    rows: Vec<Vec<redis::Value>>,
    with_dist: bool,
    with_coord: bool,
) -> Result<serde_json::Value, CamelError> {
    let mut mapped = Vec::with_capacity(rows.len());
    for row in rows {
        let mut elements = row.into_iter();
        let member = match elements.next().map(value_into_member) {
            Some(Some(member)) => member,
            _ => {
                return Err(CamelError::ProcessorError(
                    "GEOSEARCH reply row is missing a member".into(),
                ));
            }
        };
        let mut object = serde_json::Map::new();
        object.insert("member".to_string(), serde_json::json!(member));
        if with_dist {
            let Some(dist_value) = elements.next() else {
                return Err(CamelError::ProcessorError(
                    "GEOSEARCH reply row is missing the distance element (WithDist requested)"
                        .into(),
                ));
            };
            let Some(distance) = value_to_f64(&dist_value) else {
                return Err(CamelError::ProcessorError(
                    "GEOSEARCH reply row distance is not numeric (WithDist requested)".into(),
                ));
            };
            object.insert("distance".to_string(), serde_json::json!(distance));
        }
        if with_coord {
            let Some(coord_value) = elements.next() else {
                return Err(CamelError::ProcessorError(
                    "GEOSEARCH reply row is missing the coordinates element (WithCoord requested)"
                        .into(),
                ));
            };
            let (lon, lat) = parse_coord_pair(coord_value)?;
            object.insert("longitude".to_string(), serde_json::json!(round_to_6(lon)));
            object.insert("latitude".to_string(), serde_json::json!(round_to_6(lat)));
        }
        if elements.next().is_some() {
            return Err(CamelError::ProcessorError(
                "GEOSEARCH reply row has more elements than the requested flags allow".into(),
            ));
        }
        mapped.push(serde_json::Value::Object(object));
    }
    Ok(serde_json::Value::Array(mapped))
}

#[allow(clippy::too_many_arguments)] // flat args mirror the GEOSEARCH keyword order
fn build_geosearch_cmd(
    key: &str,
    longitude: f64,
    latitude: f64,
    shape: &GeoSearchShape,
    unit: &str,
    with_dist: bool,
    with_coord: bool,
    count: Option<i64>,
) -> redis::Cmd {
    let mut cmd = redis::cmd("GEOSEARCH");
    cmd.arg(key).arg("FROMLONLAT").arg(longitude).arg(latitude);
    match shape {
        GeoSearchShape::Radius(radius) => {
            cmd.arg("BYRADIUS").arg(radius);
        }
        GeoSearchShape::Box { width, height } => {
            cmd.arg("BYBOX").arg(width).arg(height);
        }
    }
    cmd.arg(unit);
    if with_dist {
        cmd.arg("WITHDIST");
    }
    if with_coord {
        cmd.arg("WITHCOORD");
    }
    if let Some(n) = count {
        cmd.arg("COUNT").arg(n);
    }
    cmd
}

async fn execute_geosearch(
    exchange: &mut Exchange,
    conn: &mut MultiplexedConnection,
) -> Result<(), CamelError> {
    let key = require_key(exchange)?;
    let longitude = require_f64_header(exchange, "CamelRedis.Longitude")?;
    validate_longitude(longitude)?;
    let latitude = require_f64_header(exchange, "CamelRedis.Latitude")?;
    validate_latitude(latitude)?;
    let shape = resolve_geo_search_shape(exchange)?;
    let unit = resolve_geo_unit(exchange)?;
    let with_dist = get_bool_header(exchange, "CamelRedis.WithDist").unwrap_or(false);
    let with_coord = get_bool_header(exchange, "CamelRedis.WithCoord").unwrap_or(false);
    let count = get_i64_header(exchange, "CamelRedis.Count")?.filter(|n| *n > 0);
    let cmd = build_geosearch_cmd(
        &key, longitude, latitude, &shape, unit, with_dist, with_coord, count,
    );

    let body = if with_dist || with_coord {
        let rows: Vec<Vec<redis::Value>> = cmd
            .query_async(conn)
            .await
            .map_err(|e| crate::transport_error::redis_error_to_camel("GEOSEARCH", e))?;
        json_from_geo_rows(rows, with_dist, with_coord)?
    } else {
        let members: Vec<String> = cmd
            .query_async(conn)
            .await
            .map_err(|e| crate::transport_error::redis_error_to_camel("GEOSEARCH", e))?;
        serde_json::json!(members)
    };
    exchange.input.body = Body::Json(body);
    Ok(())
}

pub async fn dispatch(
    cmd: &RedisCommand,
    conn: &mut MultiplexedConnection,
    exchange: &mut Exchange,
) -> Result<(), CamelError> {
    if !is_geo_command(cmd) {
        return Err(CamelError::ProcessorError("Not a geo command".into()));
    }

    match cmd {
        RedisCommand::Geoadd => execute_geoadd(exchange, conn).await,
        RedisCommand::Geopos => execute_geopos(exchange, conn).await,
        RedisCommand::Geodist => execute_geodist(exchange, conn).await,
        RedisCommand::Geohash => execute_geohash(exchange, conn).await,
        RedisCommand::Geosearch => execute_geosearch(exchange, conn).await,
        _ => unreachable!("non-geo commands rejected above"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::RedisCommand;
    use camel_component_api::{Exchange, Message};

    fn ex_with(headers: &[(&str, serde_json::Value)]) -> Exchange {
        let mut msg = Message::default();
        for (k, v) in headers {
            msg.set_header(*k, v.clone());
        }
        Exchange::new(msg)
    }

    #[test]
    fn test_is_geo_command_matches_only_geo_variants() {
        assert!(is_geo_command(&RedisCommand::Geoadd));
        assert!(is_geo_command(&RedisCommand::Geopos));
        assert!(is_geo_command(&RedisCommand::Geodist));
        assert!(is_geo_command(&RedisCommand::Geosearch));
        assert!(is_geo_command(&RedisCommand::Geohash));
        assert!(!is_geo_command(&RedisCommand::Zadd));
    }

    #[test]
    fn test_validate_latitude_rejects_out_of_range() {
        for v in [95.0, -91.0] {
            let err = validate_latitude(v).expect_err("out-of-range latitude must fail");
            assert!(
                err.to_string().contains("CamelRedis.Latitude"),
                "error must name CamelRedis.Latitude: {err}"
            );
        }
        assert!(validate_latitude(90.0).is_ok());
        assert!(validate_latitude(0.0).is_ok());
    }

    #[test]
    fn test_validate_longitude_rejects_out_of_range() {
        for v in [-181.0, 181.0] {
            let err = validate_longitude(v).expect_err("out-of-range longitude must fail");
            assert!(
                err.to_string().contains("CamelRedis.Longitude"),
                "error must name CamelRedis.Longitude: {err}"
            );
        }
        assert!(validate_longitude(180.0).is_ok());
    }

    #[test]
    fn test_require_f64_header_missing_fails() {
        let ex = Exchange::new(Message::default());
        let err =
            require_f64_header(&ex, "CamelRedis.Longitude").expect_err("missing header must fail");
        assert!(
            err.to_string().contains("CamelRedis.Longitude"),
            "error must name the header: {err}"
        );
    }

    #[test]
    fn test_require_member_missing_fails() {
        let ex = Exchange::new(Message::default());
        let err = require_member(&ex).expect_err("missing member must fail");
        assert!(
            err.to_string().contains("CamelRedis.Member"),
            "error must name CamelRedis.Member: {err}"
        );
    }

    #[test]
    fn test_require_members_missing_and_empty_fail() {
        let missing = Exchange::new(Message::default());
        let err = require_members(&missing).expect_err("missing members must fail");
        assert!(
            err.to_string().contains("CamelRedis.Members"),
            "error must name CamelRedis.Members: {err}"
        );

        let empty = ex_with(&[("CamelRedis.Members", serde_json::json!([]))]);
        let err = require_members(&empty).expect_err("empty members must fail");
        assert!(
            err.to_string().contains("CamelRedis.Members"),
            "error must name CamelRedis.Members: {err}"
        );
    }

    #[test]
    fn test_json_from_geopos_shapes_pairs_and_nulls() {
        let positions = vec![Some((13.361389, 38.115556)), None];
        assert_eq!(
            json_from_geopos(&positions),
            serde_json::json!([[13.361389, 38.115556], null])
        );
    }

    #[test]
    fn test_geoadd_rejects_bad_latitude_before_command() {
        let ex = ex_with(&[
            ("CamelRedis.Key", serde_json::json!("points")),
            ("CamelRedis.Member", serde_json::json!("palermo")),
            ("CamelRedis.Longitude", serde_json::json!(13.361389)),
            ("CamelRedis.Latitude", serde_json::json!(95.0)),
        ]);
        let err = resolve_geoadd_args(&ex).expect_err("latitude 95.0 must be rejected");
        assert!(
            err.to_string().contains("CamelRedis.Latitude"),
            "error must name CamelRedis.Latitude: {err}"
        );
    }

    #[test]
    fn test_resolve_geo_unit_defaults_and_rejects_unknown() {
        let km = ex_with(&[("CamelRedis.Unit", serde_json::json!("KM"))]);
        assert_eq!(resolve_geo_unit(&km).expect("KM must be accepted"), "km");

        let absent = Exchange::new(Message::default());
        assert_eq!(
            resolve_geo_unit(&absent).expect("absent unit must default to meters"),
            "m"
        );

        let unknown = ex_with(&[("CamelRedis.Unit", serde_json::json!("furlongs"))]);
        let err = resolve_geo_unit(&unknown).expect_err("unknown unit must fail");
        assert!(
            err.to_string().contains("CamelRedis.Unit"),
            "error must name CamelRedis.Unit: {err}"
        );
    }

    #[test]
    fn test_geodist_unit_defaults_to_meters() {
        let ex = ex_with(&[
            ("CamelRedis.Key", serde_json::json!("points")),
            ("CamelRedis.Member", serde_json::json!("a")),
            ("CamelRedis.Member2", serde_json::json!("b")),
        ]);
        let key = require_key(&ex).expect("key present");
        let m1 = require_member(&ex).expect("member present");
        let m2 = require_member2(&ex).expect("second member present");
        let unit = resolve_geo_unit(&ex).expect("unit defaults to meters when absent");
        let cmd = build_geodist_cmd(&key, &m1, &m2, unit);
        let redis::Arg::Simple(last) = cmd.args_iter().last().expect("GEODIST command has args")
        else {
            panic!("unit arg must be a simple arg");
        };
        assert_eq!(last, b"m".as_slice(), "default unit must be meters");
    }

    #[test]
    fn test_require_member2_missing_fails() {
        let ex = ex_with(&[("CamelRedis.Member", serde_json::json!("a"))]);
        let err = require_member2(&ex).expect_err("missing second member must fail");
        assert!(
            err.to_string().contains("CamelRedis.Member2"),
            "error must name CamelRedis.Member2: {err}"
        );
    }

    #[test]
    fn test_geohash_body_preserves_nulls() {
        let hashes = vec![Some("sqdtr74hyu0".into()), None];
        assert_eq!(
            json_from_geohashes(hashes),
            serde_json::json!(["sqdtr74hyu0", null])
        );
    }

    #[test]
    fn test_validate_positive_rejects_zero_and_nan() {
        for v in [0.0, f64::NAN] {
            let err = validate_positive(v, "CamelRedis.Radius")
                .expect_err("zero and NaN must be rejected");
            assert!(
                err.to_string().contains("CamelRedis.Radius"),
                "error must name CamelRedis.Radius: {err}"
            );
        }
        assert!(validate_positive(0.1, "CamelRedis.Radius").is_ok());
    }

    #[test]
    fn test_geo_search_shape_rejects_radius_and_box_together() {
        let ex = ex_with(&[
            ("CamelRedis.Radius", serde_json::json!(100)),
            ("CamelRedis.Width", serde_json::json!(50)),
            ("CamelRedis.Height", serde_json::json!(50)),
        ]);
        let err = resolve_geo_search_shape(&ex).expect_err("radius and box together must fail");
        assert!(
            err.to_string().contains("mutually exclusive"),
            "error must state mutual exclusion: {err}"
        );
    }

    #[test]
    fn test_geo_search_shape_rejects_neither() {
        let ex = ex_with(&[
            ("CamelRedis.Longitude", serde_json::json!(13.361389)),
            ("CamelRedis.Latitude", serde_json::json!(38.115556)),
        ]);
        let err = resolve_geo_search_shape(&ex).expect_err("neither radius nor box must fail");
        assert!(
            err.to_string().contains("radius or a box"),
            "error must require a radius or a box: {err}"
        );
    }

    #[test]
    fn test_geo_search_shape_rejects_non_positive_radius() {
        let ex = ex_with(&[("CamelRedis.Radius", serde_json::json!(0))]);
        let err = resolve_geo_search_shape(&ex).expect_err("radius 0 must fail");
        assert!(
            err.to_string().contains("CamelRedis.Radius"),
            "error must name CamelRedis.Radius: {err}"
        );
    }

    #[test]
    fn test_geo_search_shape_rejects_non_positive_height() {
        let ex = ex_with(&[
            ("CamelRedis.Width", serde_json::json!(50)),
            ("CamelRedis.Height", serde_json::json!(-1)),
        ]);
        let err = resolve_geo_search_shape(&ex).expect_err("height -1 must fail");
        assert!(
            err.to_string().contains("CamelRedis.Height"),
            "error must name CamelRedis.Height: {err}"
        );
    }

    #[test]
    fn test_geo_search_shape_rejects_non_numeric_width() {
        let ex = ex_with(&[
            ("CamelRedis.Width", serde_json::json!("wide")),
            ("CamelRedis.Height", serde_json::json!(50)),
        ]);
        let err = resolve_geo_search_shape(&ex).expect_err("non-numeric width must fail");
        let msg = err.to_string();
        assert!(
            msg.contains("CamelRedis.Width") && msg.contains("number"),
            "error must name CamelRedis.Width and require a number: {msg}"
        );
    }

    #[test]
    fn test_json_from_geo_rows_member_dist_coord() {
        let rows = vec![vec![
            redis::Value::BulkString(b"Palermo".to_vec()),
            redis::Value::BulkString(b"123.456".to_vec()),
            redis::Value::Array(vec![
                redis::Value::BulkString(b"13.3613893389".to_vec()),
                redis::Value::BulkString(b"38.1155563955".to_vec()),
            ]),
        ]];
        let body = json_from_geo_rows(rows, true, true).expect("valid rows must parse");
        assert_eq!(
            body,
            serde_json::json!([{
                "member": "Palermo",
                "distance": 123.456,
                "longitude": 13.361389,
                "latitude": 38.115556
            }])
        );
        let row = body.as_array().expect("body is an array")[0]
            .as_object()
            .expect("row is an object");
        assert!(row["member"].is_string(), "member must be a plain string");
        assert!(row["distance"].is_number(), "distance must be a number");
        assert!(row["longitude"].is_number(), "longitude must be a number");
        assert!(row["latitude"].is_number(), "latitude must be a number");

        let single = json_from_geo_rows(
            vec![vec![redis::Value::BulkString(b"Catania".to_vec())]],
            false,
            false,
        )
        .expect("single-element row without extras must parse");
        assert_eq!(single, serde_json::json!([{"member": "Catania"}]));
    }

    fn geosearch_cmd_args(headers: &[(&str, serde_json::Value)]) -> Vec<Vec<u8>> {
        let ex = ex_with(headers);
        let key = require_key(&ex).expect("key present");
        let longitude = require_f64_header(&ex, "CamelRedis.Longitude").expect("longitude present");
        let latitude = require_f64_header(&ex, "CamelRedis.Latitude").expect("latitude present");
        let shape = resolve_geo_search_shape(&ex).expect("shape resolves");
        let unit = resolve_geo_unit(&ex).expect("unit defaults to meters");
        let count = get_i64_header(&ex, "CamelRedis.Count")
            .expect("count header parses")
            .filter(|n| *n > 0);
        let cmd = build_geosearch_cmd(&key, longitude, latitude, &shape, unit, false, false, count);
        cmd.args_iter()
            .map(|arg| match arg {
                redis::Arg::Simple(bytes) => bytes.to_vec(),
                _ => unreachable!("GEOSEARCH has no cursor arg"),
            })
            .collect()
    }

    #[test]
    fn test_geosearch_count_header_controls_count_arg() {
        let base = [
            ("CamelRedis.Key", serde_json::json!("points")),
            ("CamelRedis.Longitude", serde_json::json!(13.361389)),
            ("CamelRedis.Latitude", serde_json::json!(38.115556)),
            ("CamelRedis.Radius", serde_json::json!(100)),
        ];

        let mut with_count = base.to_vec();
        with_count.push(("CamelRedis.Count", serde_json::json!(5)));
        let args = geosearch_cmd_args(&with_count);
        let pos = args
            .iter()
            .position(|arg| arg.as_slice() == b"COUNT")
            .expect("COUNT arg must be present when CamelRedis.Count is set");
        assert_eq!(
            args[pos + 1],
            b"5".as_slice(),
            "COUNT must be followed by the requested count"
        );

        let args = geosearch_cmd_args(&base);
        assert!(
            !args.iter().any(|arg| arg.as_slice() == b"COUNT"),
            "COUNT arg must be absent when CamelRedis.Count is not set"
        );
    }

    #[test]
    fn test_json_from_geo_rows_dist_only_two_element_rows() {
        let rows = vec![vec![
            redis::Value::BulkString(b"Palermo".to_vec()),
            redis::Value::BulkString(b"190.44".to_vec()),
        ]];
        let body =
            json_from_geo_rows(rows, true, false).expect("2-element WITHDIST row must parse");
        assert_eq!(
            body,
            serde_json::json!([{"member": "Palermo", "distance": 190.44}])
        );
    }
}
