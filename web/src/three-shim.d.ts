// Ambient module shims for the CDN-hosted Three.js ESM build.
// These let `tsc` emit JavaScript without a local `node_modules` install;
// the bare specifiers are resolved at runtime by the importmap in index.html.
declare module 'three';
declare module 'three/addons/controls/OrbitControls.js';
