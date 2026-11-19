# mrs_uav_gazebo_simulator (derived)

From https://github.com/ctu-mrs/mrs_uav_gazebo_simulator, branch `ros1`, commit
`e6b3488e764250de293756891fd449d3902cccf9` (e6b3488).
Licence: `LICENSE` in this directory (BSD 3-Clause, CTU Multi-Robot Systems group).

Only the Holybro X500 v2 visual model, for viewers:

- `meshes/`: the meshes `models/mrs_robots_description/sdf/x500.sdf.jinja` uses, from
  `models/mrs_robots_description/meshes/x500v2/`. The STL files are copied unchanged.
  The four COLLADA files (`top_board`, `bottom_board`, `arm_carbon`, `prop`) are converted
  to binary STL with `assimp export -fstlb -ptv` and rotated back to Z-up; their carbon
  texture is dropped (Foxglove cannot resolve its relative path).
- `x500.urdf`: the template rendered with jinja2 and no sensors, converted once. Every
  visual of every link becomes a visual of the one `{uav}/fcu` link (all poses are
  translations and yaw, composed), Gazebo material scripts and the carbon texture become
  flat colours, and each mesh is `file://{dir}/meshes/<name>.stl`. `mrs_simulator` fills
  in the UAV name and this directory.
