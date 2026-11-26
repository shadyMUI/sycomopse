use crate::models::{AppError, AppResult, EqualizerSettings, ErrorCode};
use crate::profiles::{atomic_write, read_limited, validate_node_name, AppPaths, APP_NODE_PREFIX};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::PathBuf;

pub(crate) const FREQUENCIES: [f64; 10] = [
    31.5, 63., 125., 250., 500., 1000., 2000., 4000., 8000., 16000.,
];
pub(crate) type Curves = BTreeMap<String, EqualizerSettings>;

impl EqualizerSettings {
    pub(crate) fn validate(&self) -> AppResult<()> {
        if self
            .preamp_db
            .is_some_and(|db| !(-24.0..=0.0).contains(&db))
        {
            return Err(AppError::new(
                ErrorCode::Validation,
                "Il preamplificatore manuale deve essere tra -24 e 0 dB.",
            ));
        }
        if self
            .bands
            .iter()
            .any(|b| !(-12.0..=12.0).contains(&b.gain_db) || !(0.4..=4.0).contains(&b.q))
        {
            return Err(AppError::new(
                ErrorCode::Validation,
                "EQ: gain da -12 a +12 dB e Q da 0.4 a 4 per ciascuna delle dieci bande.",
            ));
        }
        Ok(())
    }

    fn preamp(&self) -> f64 {
        // Conservative bound for cascaded peaking filters, including overlapping wide bands.
        let db = self
            .preamp_db
            .unwrap_or_else(|| -self.bands.iter().map(|b| b.gain_db.max(0.0)).sum::<f64>());
        10.0_f64.powf(db / 20.0)
    }

    pub(crate) fn controls(&self) -> impl Iterator<Item = (String, f64)> + '_ {
        ["L", "R"].into_iter().flat_map(|side| {
            std::iter::once((format!("pre{side}:Gain 1"), self.preamp())).chain(
                self.bands.iter().enumerate().flat_map(move |(i, band)| {
                    [
                        (format!("eq{side}{i}:Gain"), band.gain_db),
                        (format!("eq{side}{i}:Q"), band.q),
                    ]
                }),
            )
        })
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Document {
    schema_version: u32,
    outputs: Curves,
}

fn invalid() -> AppError {
    AppError::new(
        ErrorCode::InvalidData,
        "Impostazioni EQ non valide: il file originale non verrà sostituito.",
    )
}

fn invalid_graph() -> AppError {
    AppError::new(
        ErrorCode::InvalidData,
        "Template PipeWire per l'equalizzatore non valido.",
    )
}

fn validate_curves(curves: &Curves) -> AppResult<()> {
    if curves.len() > 128 {
        return Err(invalid());
    }
    for (key, eq) in curves {
        validate_node_name(key)?;
        if key.starts_with(APP_NODE_PREFIX) {
            return Err(invalid());
        }
        eq.validate()?;
    }
    Ok(())
}

pub(super) fn load(paths: &AppPaths) -> AppResult<Curves> {
    let bytes = match read_limited(&paths.config_dir.join("equalizers.json"), 256 * 1024) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Curves::new()),
        Err(err) => return Err(err.into()),
    };
    let doc: Document = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
    if doc.schema_version != 1 {
        return Err(invalid());
    }
    validate_curves(&doc.outputs)?;
    Ok(doc.outputs)
}

/// A cancelled DSP operation must not publish its proposed settings on disk.
pub(super) struct PendingCurve {
    file: PathBuf,
    updated: Curves,
}

impl PendingCurve {
    pub(super) fn commit(mut self, paths: &AppPaths) -> AppResult<Curves> {
        std::fs::rename(&self.file, paths.config_dir.join("equalizers.json")).map_err(|e| {
            AppError::new(
                ErrorCode::Io,
                "EQ applicato, ma impossibile salvare le impostazioni.",
            )
            .detail(e.to_string())
        })?;
        Ok(std::mem::take(&mut self.updated))
    }
}

impl Drop for PendingCurve {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.file);
    }
}

pub(super) fn stage(
    paths: &AppPaths,
    name: &str,
    settings: &EqualizerSettings,
) -> AppResult<PendingCurve> {
    let mut curves = load(paths)?;
    curves.insert(name.into(), settings.clone());
    validate_curves(&curves)?;
    let bytes =
        serde_json::to_vec(&json!({"schemaVersion":1, "outputs":curves})).map_err(|_| invalid())?;
    if bytes.len() > 256 * 1024 {
        return Err(invalid());
    }
    let file = paths
        .config_dir
        .join(format!(".eq-{}.pending", uuid::Uuid::new_v4()));
    atomic_write(&file, &bytes)?;
    Ok(PendingCurve {
        file,
        updated: curves,
    })
}

#[derive(Default, Deserialize, Serialize)]
struct Graph {
    nodes: Vec<Value>,
    links: Vec<Value>,
    inputs: Vec<String>,
    outputs: Vec<String>,
}

/// Share the existing isolated client envelope, target pinning and plugin policy. In
/// stereo mode replace the SOFA graph entirely; in spatial mode process its two ear signals.
pub(crate) fn render(config: String, surround: bool, eq: &EqualizerSettings) -> AppResult<String> {
    eq.validate()?;
    if !eq.enabled {
        return if surround {
            Ok(config)
        } else {
            Err(AppError::new(
                ErrorCode::Validation,
                "Nessun effetto audio richiesto.",
            ))
        };
    }
    let (comments, body) = config.split_once("\n{").ok_or_else(invalid_graph)?;
    let mut root: Value =
        serde_json::from_str(&format!("{{{body}")).map_err(|_| invalid_graph())?;
    if !surround {
        root["context.properties"]["application.name"] = json!("Sycomopse Equalizer");
    }
    let filter = root["context.modules"]
        .as_array_mut()
        .and_then(|m| {
            m.iter_mut()
                .find(|m| m["name"] == "libpipewire-module-filter-chain")
        })
        .ok_or_else(invalid_graph)?;
    let args = &mut filter["args"];
    let mut graph: Graph = if surround {
        serde_json::from_value(args["filter.graph"].take()).map_err(|_| invalid_graph())?
    } else {
        for key in ["node.description", "media.name"] {
            args[key] = json!("Sycomopse Stereo EQ");
        }
        args["capture.props"]["node.description"] = json!("Sycomopse Stereo EQ");
        args["capture.props"]["audio.channels"] = json!(2);
        args["capture.props"]["audio.position"] = json!(["FL", "FR"]);
        Graph::default()
    };
    let ears = std::mem::take(&mut graph.outputs);
    let preamp = eq.preamp();
    for (i, side) in ["L", "R"].iter().enumerate() {
        let pre = format!("pre{side}");
        graph
            .nodes
            .push(json!({"type":"builtin","label":"mixer","name":pre,"control":{"Gain 1":preamp}}));
        let input = format!("{pre}:In 1");
        if surround {
            graph
                .links
                .push(json!({"output":ears.get(i).ok_or_else(invalid_graph)?,"input":input}));
        } else {
            graph.inputs.push(input);
        }
        let mut previous = format!("{pre}:Out");
        for (j, (band, frequency)) in eq.bands.iter().zip(FREQUENCIES).enumerate() {
            let name = format!("eq{side}{j}");
            graph.nodes.push(json!({"type":"builtin","name":name,"label":"bq_peaking","control":{"Freq":frequency,"Gain":band.gain_db,"Q":band.q}}));
            graph
                .links
                .push(json!({"output":previous,"input":format!("{name}:In")}));
            previous = format!("{name}:Out");
        }
        graph.outputs.push(previous);
    }
    args["filter.graph"] = serde_json::to_value(graph).map_err(|_| invalid_graph())?;
    Ok(format!(
        "{comments}\n{}",
        serde_json::to_string_pretty(&root).map_err(|_| invalid_graph())?
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::surround::render_playback_config;
    use std::path::Path;

    #[test]
    fn bands_are_bounded_and_preamp_protects_overlapping_boosts() {
        let mut eq = EqualizerSettings::default();
        eq.bands[2].gain_db = -12.;
        eq.bands[3].gain_db = 6.;
        eq.bands[4].gain_db = 3.;
        assert!((20.0 * eq.preamp().log10() + 9.0).abs() < 1e-8);
        assert!(eq.validate().is_ok());
        eq.preamp_db = Some(0.0);
        assert_eq!(eq.preamp(), 1.0);
        assert_eq!(eq.controls().count(), 42);
        for invalid in [0.01, -24.01, f64::NAN, f64::INFINITY] {
            eq.preamp_db = Some(invalid);
            assert!(eq.validate().is_err());
        }
        eq.preamp_db = None;
        for (gain, q) in [
            (12.01, 1.0),
            (-12.01, 1.0),
            (0., 4.01),
            (0., 0.39),
            (f64::NAN, 1.0),
            (0., f64::INFINITY),
        ] {
            eq.bands[0].gain_db = gain;
            eq.bands[0].q = q;
            assert!(eq.validate().is_err());
        }
        assert!(
            serde_json::from_value::<EqualizerSettings>(json!({"enabled":true,"bands":[]}))
                .is_err()
        );
    }

    #[test]
    fn both_ears_get_their_own_ten_filters_after_headroom_and_after_spatialization() {
        let target = "alsa_output.\"target\\{{SESSION}}";
        let mut eq = EqualizerSettings {
            enabled: true,
            ..Default::default()
        };
        eq.bands[5].gain_db = 6.;
        eq.bands[5].q = 1.1;
        for spatial in [false, true] {
            let sofa = spatial.then_some(Path::new("/test/HRTF.sofa"));
            let config =
                render_playback_config(target, sofa, "input", "playback", "session", &eq).unwrap();
            let config: Value =
                serde_json::from_str(&config[config.find("\n{").unwrap()..]).unwrap();
            let args = &config["context.modules"]
                .as_array()
                .unwrap()
                .last()
                .unwrap()["args"];
            assert_eq!(args["playback.props"]["target.object"], target);
            assert_eq!(args["playback.props"]["node.dont-fallback"], true);
            assert_eq!(
                args["capture.props"]["audio.channels"],
                if spatial { 8 } else { 2 }
            );
            let graph: Graph = serde_json::from_value(args["filter.graph"].clone()).unwrap();
            assert_eq!(
                graph
                    .nodes
                    .iter()
                    .filter(|n| n["label"] == "bq_peaking")
                    .count(),
                20
            );
            assert_eq!(
                graph.nodes.iter().filter(|n| n["type"] == "sofa").count(),
                if spatial { 8 } else { 0 }
            );
            for (i, side) in ["L", "R"].iter().enumerate() {
                assert_eq!(graph.outputs[i], format!("eq{side}9:Out"));
                let pre = graph
                    .nodes
                    .iter()
                    .find(|n| n["name"] == format!("pre{side}"))
                    .unwrap();
                assert!(
                    (pre["control"]["Gain 1"].as_f64().unwrap() - 10.0_f64.powf(-6. / 20.)).abs()
                        < 1e-8
                );
                if spatial {
                    assert!(graph.links.contains(&json!({"output":format!("mix{side}:Out"),"input":format!("pre{side}:In 1")})));
                } else {
                    assert_eq!(graph.inputs[i], format!("pre{side}:In 1"));
                }
                let band = graph
                    .nodes
                    .iter()
                    .find(|n| n["name"] == format!("eq{side}5"))
                    .unwrap();
                assert_eq!(band["control"], json!({"Freq":1000.,"Gain":6.,"Q":1.1}));
            }
        }
    }
}
