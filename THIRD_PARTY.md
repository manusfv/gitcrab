# Third-party material

GitCrab's original code and assets retain the MIT license in LICENSE. Native packages include DEPENDENCY-LICENSES.txt generated from the resolved Cargo manifests and upstream license/notice files, including build dependencies. Cargo.lock records exact registry checksums and versions.

The native host uses GPUI 0.2.2 (Zed Industries, Apache-2.0) and gpui-component 0.5.1 (Longbridge, Apache-2.0). The small xattr 0.2.3 compatibility patch retains its MIT/Apache licenses and provenance under vendor/xattr.

The vendored gpui-component 0.5.1 keeps its upstream Apache-2.0 license. Its local changes give enabled buttons and menu entries a pointer cursor, including dialog controls created by the library. See vendor/gpui-component/GITCRAB-PATCH.md for provenance and patch scope.

GitCrab’s styling is independently implemented. the reference app screenshots are observation evidence. System fonts are resolved on the user's operating system.
