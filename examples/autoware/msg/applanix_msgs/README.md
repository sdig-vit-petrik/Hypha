# applanix_msgs

`NavigationSolutionGsof49` and the messages it holds (`GpsTimeGsof`, `StatusGsof`, `LLA`,
`NED`) of https://github.com/autowarefoundation/applanix, branch `main`, commit
`6689c1ba2796b2c6cc77a6e9c5dd1c1507d8f862` (6689c1b); the definitions are unchanged since
the repository's first commit (`a174f7f`, 2022-08-01). The Leo Drive Isuzu bags (#46) record
the Applanix INS solution as GSOF 49 on `/lvx_client/gsof/ins_solution_49`; `hypha convert`
reads it with these definitions (`--msg-path`). Licence: `LICENSE` (Apache 2.0).

The files are unchanged: their ROS 2 definitions are valid ROS 1 syntax.
