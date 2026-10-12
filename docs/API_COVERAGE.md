# API coverage ledger

Pinned upstream: `38a886906ed175d0205bf48b6dc03f54aeb4a81a`. Every operation named below is implemented and tested as of 2026-10-12 ([IMPLEMENTATION_STATUS.md](IMPLEMENTATION_STATUS.md) maps operations to tests).

Every public declaration in the scientific modules is listed, including module-accessible base classes and the MIST readers. Inherited methods are listed once at their defining class; the `bases` fields in the JSON inventory retain inheritance. Constructors map to typed requests, properties to inspection/query results, operators to composition operations. Python pickle methods map to portable artifact persistence, not pickle compatibility. `MISTIsochrone.from_parsec` is an upstream rejection path; the Rust import dispatcher routes PARSEC to the generic isochrone type.

All operation IDs below have both a CLI route (dots become spaces) and an MCP tool (`artpop_` plus dots/hyphens replaced with underscores). Slash-separated entries describe several related operations, not one command. Detailed contracts are in [CLI_MCP_CONTRACT.md](CLI_MCP_CONTRACT.md).

Logging internals and Python import/representation mechanics become Rust tracing and typed display/serialization. Third-party object APIs and arbitrary Python keyword forwarding are not independent ArtPop scientific features; supported replacements are specified explicitly.

## `filters`

| Upstream declaration | Kind | Proposed operation(s) |
| --- | --- | --- |
| [`phot_system_list`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/filters.py#L23) | constant | `filters.list` |
| [`FilterSystem`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/filters.py#L29) | class | `filters.import` |
| [`FilterSystem.__init__`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/filters.py#L47) | method | `filters.import` |
| [`FilterSystem.effective_throughput`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/filters.py#L63) | method | `filters.inspect` |
| [`FilterSystem.lam_eff`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/filters.py#L81) | method | `filters.inspect` |
| [`FilterSystem.lam_pivot`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/filters.py#L101) | method | `filters.inspect` |
| [`FilterSystem.dlam`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/filters.py#L121) | method | `filters.inspect` |
| [`ZeroPointConverter`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/filters.py#L141) | class | `filters.convert` |
| [`ZeroPointConverter.__init__`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/filters.py#L152) | method | `filters.convert` |
| [`ZeroPointConverter.to_vega`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/filters.py#L157) | method | `filters.convert` |
| [`ZeroPointConverter.to_ab`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/filters.py#L178) | method | `filters.convert` |
| [`ZeroPointConverter.to_st`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/filters.py#L199) | method | `filters.convert` |
| [`ZeroPointConverter.color_to_vega`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/filters.py#L220) | method | `filters.convert` |
| [`ZeroPointConverter.color_to_ab`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/filters.py#L240) | method | `filters.convert` |
| [`ZeroPointConverter.color_to_st`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/filters.py#L260) | method | `filters.convert` |
| [`get_filter_names`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/filters.py#L281) | function | `filters.list` |
| [`get_filter_properties`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/filters.py#L311) | function | `filters.inspect` |
| [`phot_system_lookup`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/filters.py#L322) | function | `filters.list` |
| [`load_zero_point_converter`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/filters.py#L347) | function | `filters.convert` |

## `image.imager`

| Upstream declaration | Kind | Proposed operation(s) |
| --- | --- | --- |
| [`Observation`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/image/imager.py#L24) | class | `observation.inspect / artifact.import` |
| [`Observation.to_pickle`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/image/imager.py#L26) | method | `artifact.export / artifact.import / artifact.clone` |
| [`Observation.from_pickle`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/image/imager.py#L33) | method | `artifact.export / artifact.import / artifact.clone` |
| [`Observation.to_fits`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/image/imager.py#L40) | method | `artifact.export` |
| [`Observation.copy`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/image/imager.py#L58) | method | `artifact.export / artifact.import / artifact.clone` |
| [`IdealObservation`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/image/imager.py#L63) | class | `observation.inspect / artifact.import` |
| [`IdealObservation.__init__`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/image/imager.py#L76) | method | `observation.inspect / artifact.import` |
| [`ArtObservation`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/image/imager.py#L82) | class | `observation.inspect / artifact.import` |
| [`ArtObservation.__init__`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/image/imager.py#L105) | method | `observation.inspect / artifact.import` |
| [`fnu_from_AB_mag`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/image/imager.py#L120) | function | `imager.convert` |
| [`Imager`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/image/imager.py#L128) | class | `imager.inspect` |
| [`Imager.inject_stars`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/image/imager.py#L141) | method | `image.histogram` |
| [`Imager.apply_seeing`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/image/imager.py#L176) | method | `image.convolve` |
| [`Imager.inject_smooth_model`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/image/imager.py#L208) | method | `image.smooth` |
| [`Imager.observe`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/image/imager.py#L212) | method | `observe.artificial` |
| [`IdealImager`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/image/imager.py#L217) | class | `imager.inspect` |
| [`IdealImager.inject_smooth_model`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/image/imager.py#L227) | method | `image.smooth` |
| [`IdealImager.observe`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/image/imager.py#L261) | method | `observe.ideal` |
| [`ArtImager`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/image/imager.py#L295) | class | `imager.inspect` |
| [`ArtImager.__init__`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/image/imager.py#L347) | method | `imager.inspect` |
| [`ArtImager.area`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/image/imager.py#L396) | property | `imager.inspect` |
| [`ArtImager.mag_to_counts`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/image/imager.py#L401) | method | `imager.convert` |
| [`ArtImager.sb_to_counts_per_pixel`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/image/imager.py#L437) | method | `imager.convert` |
| [`ArtImager.calibration`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/image/imager.py#L482) | method | `imager.convert` |
| [`ArtImager.inject_smooth_model`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/image/imager.py#L515) | method | `image.smooth` |
| [`ArtImager.observe`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/image/imager.py#L554) | method | `observe.artificial` |

## `image.psf`

| Upstream declaration | Kind | Proposed operation(s) |
| --- | --- | --- |
| [`gaussian_psf`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/image/psf.py#L22) | function | `psf.build` |
| [`moffat_psf`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/image/psf.py#L82) | function | `psf.build` |

## `source`

| Upstream declaration | Kind | Proposed operation(s) |
| --- | --- | --- |
| [`Source`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/source.py#L30) | class | `source.build` |
| [`Source.__init__`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/source.py#L50) | method | `source.build` |
| [`Source.x`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/source.py#L70) | property | `source.query` |
| [`Source.y`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/source.py#L75) | property | `source.query` |
| [`Source.num_stars`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/source.py#L80) | property | `source.inspect` |
| [`Source.copy`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/source.py#L84) | method | `artifact.export / artifact.import / artifact.clone` |
| [`Source.select_stars`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/source.py#L88) | method | `source.query` |
| [`Source.__add__`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/source.py#L94) | method | `source.compose` |
| [`CompositeSource`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/source.py#L120) | class | `source.compose` |
| [`SersicSP`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/source.py#L125) | class | `source.build` |
| [`SersicSP.__init__`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/source.py#L162) | method | `source.build` |
| [`SersicSP.mag_to_image_amplitude`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/source.py#L202) | method | `source.amplitude` |
| [`MISTSersicSSP`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/source.py#L247) | class | `source.build` |
| [`MISTSersicSSP.__init__`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/source.py#L334) | method | `source.build` |
| [`PlummerSP`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/source.py#L358) | class | `source.build` |
| [`PlummerSP.__init__`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/source.py#L384) | method | `source.build` |
| [`PlummerSP.mag_to_image_amplitude`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/source.py#L417) | method | `source.amplitude` |
| [`MISTPlummerSSP`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/source.py#L447) | class | `source.build` |
| [`MISTPlummerSSP.__init__`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/source.py#L520) | method | `source.build` |
| [`UniformSSP`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/source.py#L543) | class | `source.build` |
| [`UniformSSP.__init__`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/source.py#L591) | method | `source.build` |
| [`UniformSSP.mag_to_image_amplitude`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/source.py#L642) | method | `source.amplitude` |
| [`MISTUniformSSP`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/source.py#L671) | class | `source.build` |
| [`MISTUniformSSP.__init__`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/source.py#L729) | method | `source.build` |

## `space.models`

| Upstream declaration | Kind | Proposed operation(s) |
| --- | --- | --- |
| [`Plummer2D`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/space/models.py#L9) | class | `spatial.evaluate` |
| [`Plummer2D.amplitude`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/space/models.py#L25) | class attribute | `spatial.evaluate` |
| [`Plummer2D.scale_radius`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/space/models.py#L26) | class attribute | `spatial.evaluate` |
| [`Plummer2D.x_0`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/space/models.py#L27) | class attribute | `spatial.evaluate` |
| [`Plummer2D.y_0`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/space/models.py#L28) | class attribute | `spatial.evaluate` |
| [`Plummer2D.evaluate`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/space/models.py#L31) | method | `spatial.evaluate` |
| [`Constant2D`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/space/models.py#L37) | class | `spatial.evaluate` |
| [`Constant2D.amplitude`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/space/models.py#L47) | class attribute | `spatial.evaluate` |
| [`Constant2D.evaluate`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/space/models.py#L50) | method | `spatial.evaluate` |

## `space.sample`

| Upstream declaration | Kind | Proposed operation(s) |
| --- | --- | --- |
| [`xy_from_grid`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/space/sample.py#L21) | function | `spatial.sample` |
| [`sersic_xy`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/space/sample.py#L114) | function | `spatial.sample` |
| [`plummer_xy`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/space/sample.py#L201) | function | `spatial.sample` |
| [`uniform_xy`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/space/sample.py#L281) | function | `spatial.sample` |

## `stars._read_mist_models`

| Upstream declaration | Kind | Proposed operation(s) |
| --- | --- | --- |
| [`IsoReader`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/stars/_read_mist_models.py#L11) | class | `isochrone.import / isochrone.inspect` |
| [`IsoReader.__init__`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/stars/_read_mist_models.py#L19) | method | `isochrone.import / isochrone.inspect` |
| [`IsoReader.read_iso_file`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/stars/_read_mist_models.py#L49) | method | `isochrone.import / isochrone.inspect` |
| [`IsoReader.age_index`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/stars/_read_mist_models.py#L88) | method | `isochrone.import / isochrone.inspect` |
| [`IsoCmdReader`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/stars/_read_mist_models.py#L108) | class | `isochrone.import / isochrone.inspect` |
| [`IsoCmdReader.__init__`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/stars/_read_mist_models.py#L113) | method | `isochrone.import / isochrone.inspect` |
| [`IsoCmdReader.read_isocmd_file`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/stars/_read_mist_models.py#L144) | method | `isochrone.import / isochrone.inspect` |
| [`IsoCmdReader.age_index`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/stars/_read_mist_models.py#L186) | method | `isochrone.import / isochrone.inspect` |

## `stars.imf`

| Upstream declaration | Kind | Proposed operation(s) |
| --- | --- | --- |
| [`kroupa_params`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/stars/imf.py#L16) | constant | `imf.evaluate` |
| [`scalo_params`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/stars/imf.py#L17) | constant | `imf.evaluate` |
| [`salpeter_params`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/stars/imf.py#L18) | constant | `imf.evaluate` |
| [`imf_params_dict`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/stars/imf.py#L19) | constant | `imf.evaluate` |
| [`kroupa`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/stars/imf.py#L24) | function | `imf.evaluate` |
| [`scalo`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/stars/imf.py#L54) | function | `imf.evaluate` |
| [`salpeter`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/stars/imf.py#L80) | function | `imf.evaluate` |
| [`sample_imf`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/stars/imf.py#L106) | function | `imf.sample` |
| [`build_galaxy`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/stars/imf.py#L156) | function | `imf.build` |
| [`IMFIntegrator`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/stars/imf.py#L228) | class | `imf.evaluate` |
| [`IMFIntegrator.__init__`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/stars/imf.py#L247) | method | `imf.evaluate` |
| [`IMFIntegrator.weights`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/stars/imf.py#L290) | method | `imf.evaluate` |
| [`IMFIntegrator.func`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/stars/imf.py#L335) | method | `imf.evaluate` |
| [`IMFIntegrator.m_func`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/stars/imf.py#L353) | method | `imf.evaluate` |
| [`IMFIntegrator.integrate`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/stars/imf.py#L396) | method | `imf.integrate` |
| [`IMFIntegrator.m_integrate`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/stars/imf.py#L461) | method | `imf.integrate` |

## `stars.isochrones`

| Upstream declaration | Kind | Proposed operation(s) |
| --- | --- | --- |
| [`Isochrone`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/stars/isochrones.py#L26) | class | `isochrone.import` |
| [`Isochrone.__init__`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/stars/isochrones.py#L49) | method | `isochrone.import` |
| [`Isochrone.m_min`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/stars/isochrones.py#L66) | property | `isochrone.inspect` |
| [`Isochrone.m_max`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/stars/isochrones.py#L71) | property | `isochrone.inspect` |
| [`Isochrone.filters`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/stars/isochrones.py#L76) | property | `isochrone.inspect` |
| [`Isochrone.from_parsec`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/stars/isochrones.py#L81) | method | `isochrone.import` |
| [`Isochrone.interpolate`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/stars/isochrones.py#L146) | method | `isochrone.interpolate` |
| [`Isochrone.mag_to_mass`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/stars/isochrones.py#L174) | method | `isochrone.invert` |
| [`Isochrone.calculate_mag_limit`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/stars/isochrones.py#L223) | method | `isochrone.magnitude-limit` |
| [`Isochrone.nearest_mini`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/stars/isochrones.py#L283) | method | `isochrone.nearest` |
| [`Isochrone.imf_weights`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/stars/isochrones.py#L303) | method | `isochrone.weights` |
| [`Isochrone.ssp_color`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/stars/isochrones.py#L363) | method | `isochrone.integrated` |
| [`Isochrone.ssp_sbf_mag`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/stars/isochrones.py#L392) | method | `isochrone.integrated` |
| [`Isochrone.ssp_mag`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/stars/isochrones.py#L420) | method | `isochrone.integrated` |
| [`Isochrone.ssp_mean_star_mass`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/stars/isochrones.py#L448) | method | `isochrone.integrated` |
| [`Isochrone.ssp_surviving_mass`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/stars/isochrones.py#L474) | method | `isochrone.integrated` |
| [`fetch_mist_iso_cmd`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/stars/isochrones.py#L528) | function | `isochrone.mist` |
| [`MISTIsochrone`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/stars/isochrones.py#L591) | class | `isochrone.mist` |
| [`MISTIsochrone.__init__`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/stars/isochrones.py#L633) | method | `isochrone.mist` |
| [`MISTIsochrone.isochrone_full`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/stars/isochrones.py#L692) | property | `isochrone.inspect` |
| [`MISTIsochrone.from_parsec`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/stars/isochrones.py#L697) | method | `isochrone.import` |

## `stars.populations`

| Upstream declaration | Kind | Proposed operation(s) |
| --- | --- | --- |
| [`StellarPopulation`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/stars/populations.py#L22) | class | `population.build` |
| [`StellarPopulation.__init__`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/stars/populations.py#L44) | method | `population.build` |
| [`StellarPopulation.build_pop`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/stars/populations.py#L50) | method | `population.build` |
| [`StellarPopulation.dist_mod`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/stars/populations.py#L55) | property | `population.inspect` |
| [`StellarPopulation.num_pops`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/stars/populations.py#L60) | property | `population.inspect` |
| [`StellarPopulation.total_initial_live_mass`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/stars/populations.py#L67) | property | `population.inspect` |
| [`StellarPopulation.num_stars`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/stars/populations.py#L72) | property | `population.inspect` |
| [`StellarPopulation.abs_mag_table`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/stars/populations.py#L77) | property | `population.query` |
| [`StellarPopulation.mag_table`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/stars/populations.py#L82) | property | `population.query` |
| [`StellarPopulation.a_lam`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/stars/populations.py#L90) | property | `population.update / population.inspect` |
| [`StellarPopulation.a_lam`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/stars/populations.py#L95) | setter | `population.update / population.inspect` |
| [`StellarPopulation.to_pickle`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/stars/populations.py#L120) | method | `artifact.export / artifact.import / artifact.clone` |
| [`StellarPopulation.from_pickle`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/stars/populations.py#L127) | method | `artifact.export / artifact.import / artifact.clone` |
| [`StellarPopulation.copy`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/stars/populations.py#L146) | method | `artifact.export / artifact.import / artifact.clone` |
| [`StellarPopulation.set_distance`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/stars/populations.py#L149) | method | `population.update / population.inspect` |
| [`StellarPopulation.star_mags`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/stars/populations.py#L161) | method | `population.query` |
| [`StellarPopulation.mag_integrated_component`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/stars/populations.py#L183) | method | `population.photometry` |
| [`StellarPopulation.sbf_mag`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/stars/populations.py#L205) | method | `population.photometry` |
| [`StellarPopulation.integrated_color`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/stars/populations.py#L235) | method | `population.photometry` |
| [`StellarPopulation.mean_mag`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/stars/populations.py#L269) | method | `population.photometry` |
| [`StellarPopulation.total_mag`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/stars/populations.py#L298) | method | `population.photometry` |
| [`SSP`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/stars/populations.py#L326) | class | `population.build` |
| [`SSP.__init__`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/stars/populations.py#L382) | method | `population.build` |
| [`SSP.build_pop`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/stars/populations.py#L395) | method | `population.build` |
| [`SSP.sample_fraction`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/stars/populations.py#L556) | method | `population.sample-fraction` |
| [`SSP.__add__`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/stars/populations.py#L595) | method | `population.compose` |
| [`MISTSSP`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/stars/populations.py#L701) | class | `population.build` |
| [`MISTSSP.phases`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/stars/populations.py#L771) | class attribute | `population.phases` |
| [`MISTSSP.__init__`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/stars/populations.py#L774) | method | `population.build` |
| [`MISTSSP.select_phase`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/stars/populations.py#L806) | method | `population.phases` |
| [`MISTSSP.get_star_phases`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/stars/populations.py#L855) | method | `population.phases` |
| [`CompositePopulation`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/stars/populations.py#L865) | class | `population.compose` |
| [`CompositePopulation.__init__`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/stars/populations.py#L870) | method | `population.compose` |
| [`constant_sb_stars_per_pix`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/stars/populations.py#L893) | function | `population.surface-density` |

## `util`

| Upstream declaration | Kind | Proposed operation(s) |
| --- | --- | --- |
| [`check_random_state`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/util.py#L25) | function | `system.validate` |
| [`check_units`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/util.py#L64) | function | `system.validate` |
| [`check_odd`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/util.py#L92) | function | `system.validate` |
| [`check_xy_dim`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/util.py#L104) | function | `system.validate` |
| [`embed_slices`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/util.py#L130) | function | `image.embed` |
| [`fetch_mist_grid_if_needed`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/util.py#L171) | function | `data.fetch` |

## `visualization`

| Upstream declaration | Kind | Proposed operation(s) |
| --- | --- | --- |
| [`mpl_style`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/visualization.py#L9) | constant | `image.preview` |
| [`jpg_style`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/visualization.py#L45) | constant | `image.preview` |
| [`show_image`](https://github.com/ArtificialStellarPopulations/ArtPop/blob/38a886906ed175d0205bf48b6dc03f54aeb4a81a/src/artpop/visualization.py#L85) | function | `image.preview` |

