//! LiDAR point clouds (LAS, LAZ, COPC) as a source: an evenly spread sample of the file's points is
//! read once ([`crate::lidar`]), and views are answered from it, so projects save the file's path
//! rather than its points.

use super::{Batch, FeatureSource, Request, SourceInfo};
use crate::Result;
use crate::{Dataset, Feature, Geometry};
use rustgis_geom::Extent;
use std::path::Path;

pub struct LidarSource {
    info: SourceInfo,
    features: Vec<Feature>,
}

impl LidarSource {
    pub fn open_path(path: &Path, name: &str) -> Result<LidarSource> {
        let f = std::io::BufReader::new(std::fs::File::open(path)?);
        Self::from_read(crate::lidar::read(f, name, None)?)
    }

    pub fn open_bytes(bytes: &[u8], name: &str) -> Result<LidarSource> {
        Self::from_read(crate::lidar::read_bytes(bytes, name, None)?)
    }

    fn from_read((ds, report): (Dataset, crate::ReadReport)) -> Result<LidarSource> {
        let Dataset { name, geometry_type, fields, features, .. } = ds;
        let extent = features.iter().filter_map(|f| f.geometry.as_ref()).fold(Extent::EMPTY, |e, g| e.union(&g.extent()));
        // The reader's messages start with the layer's name, which the source's notes add.
        let notes = report.messages.iter().map(|m| m.strip_prefix(&format!("{name}: ")).unwrap_or(m).to_string()).collect();
        let info = SourceInfo {
            name,
            format: "LiDAR",
            geometry_type,
            fields,
            extent,
            count: Some(features.len() as u64),
            crs: "EPSG:4326".into(),
            indexed: false,
            notes,
        };
        Ok(LidarSource { info, features: features.into_vec() })
    }
}

impl FeatureSource for LidarSource {
    fn info(&self) -> &SourceInfo {
        &self.info
    }

    fn read(&self, req: &Request) -> Result<Batch> {
        let inside = |f: &&Feature| match (req.extent, &f.geometry) {
            (None, _) => true,
            (Some(e), Some(Geometry::Point(c))) => c.x >= e.min_x && c.x <= e.max_x && c.y >= e.min_y && c.y <= e.max_y,
            (Some(e), Some(g)) => e.intersects(&g.extent()),
            (Some(_), None) => false,
        };
        // One pass over the points (references, not copies), then every n-th match past the
        // limit, so the view stays covered evenly.
        let hits: Vec<&Feature> = self.features.iter().filter(inside).collect();
        let matched = hits.len();
        let limit = req.limit.max(1);
        let step = matched.div_ceil(limit).max(1);
        let features: Vec<Feature> = hits.into_iter().step_by(step).take(limit).cloned().collect();
        Ok(Batch { complete: features.len() == matched, matched: matched as u64, features, skipped: 0 })
    }
}
