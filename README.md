# Storm picker

Kind of "inspired" by persistent homology, but not as sophisticated, doesn't track topological characteristics, just connected components.

Lets you choose time and distance separation thresholds and generates clusters of points from that (where there is a route between any two points where each hop is within threshold).

Also ascribes a "stability region" to said clusters, meaning the (2D) space of thresholds supporting clusters within a given Jaccard tolerance of the selected cluster.

The stability percentage reported by the app is the ratio of the stability region area to the total area of the chosen threshold parameter space.

Loosely this is how "resilient" the cluster is to change of threshold, before succumbing to a significant merge or split.

**This is currently quite hastily AI-generated and as such I can't entirely vouch for correctness. Some simplicity has been sacrificed for performance. Fine as an investigative tool but be wary if reporting quantitative results.**
