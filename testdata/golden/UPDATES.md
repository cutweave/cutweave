# Golden reference updates

Every change to `testdata/golden/expected/` comes with an entry here, in the same change, naming the adapter set it touches and the set's digest after the change. `cargo xtask golden --update --i-know` writes the entry (edit its reason if it says none, keep the digest). `cargo xtask golden --check-refs` fails on a changed set without a matching entry, and CI also wants the `golden-update` pull-request label. See [README.md](README.md).

- 2026-11-24 · `dx12-warp` · digest `d3a0708bd5ccf7a0` · 18 of 18 references written (new set) · Microsoft Basic Render Driver (10.0.26100.9818) · FFmpeg n8.1.3-39-g660b5aa7db-20261123 · reason: first reference set (WP-1O.01)
- 2026-11-24 · `dx12-nvidia-geforce-rtx-3080-laptop-gpu` · digest `4371f8431845c1f4` · 18 of 18 references written (new set) · NVIDIA GeForce RTX 3080 Laptop GPU (32.0.16.1692) · FFmpeg n8.1.3-39-g660b5aa7db-20261123 · reason: first reference set (WP-1O.01)
- 2026-11-24 · `dx12-amd-radeon-tm-graphics` · digest `c6849c8eaa48972b` · 18 of 18 references written (new set) · AMD Radeon(TM) Graphics (31.0.21925.1001) · FFmpeg n8.1.3-39-g660b5aa7db-20261123 · reason: first reference set (WP-1O.01)
