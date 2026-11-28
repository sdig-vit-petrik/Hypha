//! Standard ROS message definitions and codecs for hypha.
//!
//! Generated types for `std_msgs`, `geometry_msgs`, `sensor_msgs`, `nav_msgs`,
//! `diagnostic_msgs`, `rosgraph_msgs`, `tf2_msgs` and `hypha_msgs`. Each implements the
//! message traits from [`hypha_core`]. Each service of the packages (`nav_msgs::GetMap`,
//! `sensor_msgs::SetCameraInfo`, `std_srvs::Trigger`, ...) is a marker type with its
//! `TYPE_NAME` and `MD5`, and its two halves as messages (`GetMapRequest`, `GetMapResponse`).
//!
//! ```
//! use hypha_core::message::MessageIdentity;
//! use hypha_msgs::std_msgs::Bool;
//!
//! assert_eq!(Bool::TYPE_NAME, "std_msgs/Bool");
//! assert_eq!(Bool::SCHEMA_HASH, "8b94c1b53db61fb6aed406028ad6332a");
//! ```

#![no_std]

extern crate alloc;

include!(concat!(env!("OUT_DIR"), "/lib_generated.rs"));
