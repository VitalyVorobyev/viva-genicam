# API reference

The API reference is generated with `cargo doc` and published alongside this
book. Note that rustdoc uses the **crate** name, with underscores — `viva_gige`,
not `viva-gige`. What each crate is for is in
[How the library fits together](architecture.md).

- [`viva_genicam`](api/viva_genicam/index.html) — the facade. Start here.
- [`viva_genapi`](api/viva_genapi/index.html),
  [`viva_genapi_xml`](api/viva_genapi_xml/index.html),
  [`viva_gige`](api/viva_gige/index.html),
  [`viva_u3v`](api/viva_u3v/index.html),
  [`viva_gencp`](api/viva_gencp/index.html)
- [`viva_pfnc`](api/viva_pfnc/index.html),
  [`viva_sfnc`](api/viva_sfnc/index.html),
  [`viva_zenoh_api`](api/viva_zenoh_api/index.html),
  [`viva_camctl`](api/viva_camctl/index.html),
  [`viva_fake_gige`](api/viva_fake_gige/index.html)

The Python API is documented separately in
[Python bindings → API reference](python/api.md); it does not appear in
rustdoc.
