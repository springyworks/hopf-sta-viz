// hopf-sta-viz — GitHub Pages artist's impression.
//
// This is *not* a Maxwell solver. It is decorative Three.js geometry that
// suggests the linked electric/magnetic field tubes of a Rañada–Hopf
// electromagnetic knot (a hopfion) in Minkowski spacetime. The real
// finite-difference (FDTD) physics lives in the native Rust app:
// https://github.com/springyworks/hopf-sta-viz
//
// Authored in TypeScript; compiled to ./main.js by `tsc -p docs/tsconfig.json`.
// Do not hand-edit the generated main.js.
import * as THREE from 'three';
import { OrbitControls } from 'three/addons/controls/OrbitControls.js';
const presets = {
    fundamental: { name: 'Fundamental Hopfion', p: 1, q: 1, rad: 2.5, tube: 0.28 },
    photon: { name: 'Single Photon State', p: 1, q: 1, rad: 1.4, tube: 0.18 },
    trefoil: { name: 'Trefoil Configuration', p: 2, q: 3, rad: 2.3, tube: 0.15 },
    linked: { name: 'Linked Rings', p: 2, q: 2, rad: 2.4, tube: 0.16 },
    complex: { name: 'Complex Knot', p: 3, q: 4, rad: 2.2, tube: 0.12 },
};
const presetKeys = Object.keys(presets);
let currentPresetIndex = 0;
let demoMode = true;
let lastPresetSwitchTime = 0;
const PRESET_DURATION = 4000; // ms per preset in demo mode
const currentParams = { name: 'Demo (Auto-Cycle)', p: 1, q: 1, rad: 2.5, tube: 0.28 };
const demoBtn = document.getElementById('demo-btn');
const presetSelect = document.getElementById('preset-select');
const pSlider = document.getElementById('p-slider');
const qSlider = document.getElementById('q-slider');
const pVal = document.getElementById('p-val');
const qVal = document.getElementById('q-val');
const hudOverlay = document.getElementById('hud-overlay');
const container = document.getElementById('canvas-container');
// --- Three.js setup ---
const scene = new THREE.Scene();
scene.fog = new THREE.FogExp2(0x0a0a0c, 0.015);
const camera = new THREE.PerspectiveCamera(45, container.clientWidth / container.clientHeight, 0.1, 100);
camera.position.set(0, 0, 8);
const renderer = new THREE.WebGLRenderer({ antialias: true });
renderer.setSize(container.clientWidth, container.clientHeight);
renderer.setPixelRatio(Math.min(window.devicePixelRatio, 2));
renderer.toneMapping = THREE.ACESFilmicToneMapping;
renderer.toneMappingExposure = 1.2;
container.appendChild(renderer.domElement);
const controls = new OrbitControls(camera, renderer.domElement);
controls.enableDamping = true;
controls.dampingFactor = 0.05;
// --- Lighting ---
scene.add(new THREE.AmbientLight(0xffffff, 0.15));
const dirLight1 = new THREE.DirectionalLight(0xffffff, 0.8);
dirLight1.position.set(5, 10, 7);
scene.add(dirLight1);
const dirLight2 = new THREE.DirectionalLight(0x0055ff, 0.4);
dirLight2.position.set(-5, -5, -5);
scene.add(dirLight2);
const internalGlow = new THREE.PointLight(0x00ff88, 0.6, 15);
internalGlow.position.set(0, 0, 0);
scene.add(internalGlow);
// --- Materials: E = cobalt blue, B = emerald green ---
const eMaterial = new THREE.MeshStandardMaterial({
    color: 0x0044ff, metalness: 0.8, roughness: 0.3,
    emissive: 0x001133, emissiveIntensity: 1.2, side: THREE.DoubleSide,
});
const mMaterial = new THREE.MeshStandardMaterial({
    color: 0x00ff77, metalness: 0.8, roughness: 0.3,
    emissive: 0x002b11, emissiveIntensity: 1.2, side: THREE.DoubleSide,
});
let eMesh;
let mMesh;
const hopfGroup = new THREE.Group();
scene.add(hopfGroup);
function updateHopfionGeometry(p, q, radius, tube) {
    if (eMesh)
        hopfGroup.remove(eMesh);
    if (mMesh)
        hopfGroup.remove(mMesh);
    const eGeometry = new THREE.TorusKnotGeometry(radius, tube, 180, 16, p, q);
    eMesh = new THREE.Mesh(eGeometry, eMaterial);
    hopfGroup.add(eMesh);
    const mGeometry = new THREE.TorusKnotGeometry(radius, tube, 180, 16, p, q);
    mMesh = new THREE.Mesh(mGeometry, mMaterial);
    // Orthogonal phase shift to suggest the interlocking E/B foliation.
    mMesh.rotation.set(Math.PI / 2, 0, Math.PI / p);
    hopfGroup.add(mMesh);
}
function updateHUD() {
    const topology = currentParams.p === 1 && currentParams.q === 1 ? 'HOMOGENEOUS HOPF LINK' : 'TORUS KNOT COIL';
    const quantum = currentParams.name === 'Single Photon State'
        ? 'FOCK |n=1> ENVELOPE (LOCALIZED)'
        : 'CLASSICAL COHERENT STATE EMULATION';
    hudOverlay.textContent =
        `[ RANADA FIELD — ARTIST IMPRESSION ]
---------------------------------
PRESET CONFIG : ${currentParams.name.toUpperCase()}
POLOIDAL (p)  : ${currentParams.p}
TOROIDAL (q)  : ${currentParams.q}
BOUND RADIUS  : ${currentParams.rad.toFixed(2)}
TUBE SCALE    : ${currentParams.tube.toFixed(2)}
FIELD TOPOLOGY: ${topology}
QUANTUM STATE : ${quantum}`;
}
function syncUI(p, q) {
    pSlider.value = String(p);
    qSlider.value = String(q);
    pVal.textContent = String(p);
    qVal.textContent = String(q);
}
function applyPresetValues(preset, name) {
    currentParams.p = preset.p;
    currentParams.q = preset.q;
    currentParams.rad = preset.rad;
    currentParams.tube = preset.tube;
    currentParams.name = name;
    syncUI(currentParams.p, currentParams.q);
    updateHopfionGeometry(currentParams.p, currentParams.q, currentParams.rad, currentParams.tube);
    updateHUD();
}
function toggleDemo(forceState = null) {
    demoMode = forceState !== null ? forceState : !demoMode;
    if (demoMode) {
        demoBtn.classList.add('active');
        demoBtn.textContent = 'DEMO MODE: ON';
        presetSelect.value = 'demo';
        pSlider.disabled = true;
        qSlider.disabled = true;
        lastPresetSwitchTime = performance.now();
        currentParams.name = 'Demo (Auto-Cycle)';
    }
    else {
        demoBtn.classList.remove('active');
        demoBtn.textContent = 'DEMO MODE: OFF';
        pSlider.disabled = false;
        qSlider.disabled = false;
        if (presetSelect.value === 'demo') {
            presetSelect.value = 'fundamental';
            applyPresetValues(presets.fundamental, presets.fundamental.name);
        }
    }
}
demoBtn.addEventListener('click', () => toggleDemo());
presetSelect.addEventListener('change', (e) => {
    const val = e.target.value;
    if (val === 'demo') {
        toggleDemo(true);
    }
    else {
        toggleDemo(false);
        applyPresetValues(presets[val], presets[val].name);
    }
});
function handleManualSliderChange() {
    currentParams.p = parseInt(pSlider.value, 10);
    currentParams.q = parseInt(qSlider.value, 10);
    currentParams.name = 'Manual Override';
    pVal.textContent = String(currentParams.p);
    qVal.textContent = String(currentParams.q);
    updateHopfionGeometry(currentParams.p, currentParams.q, currentParams.rad, currentParams.tube);
    updateHUD();
}
pSlider.addEventListener('input', handleManualSliderChange);
qSlider.addEventListener('input', handleManualSliderChange);
window.addEventListener('resize', () => {
    camera.aspect = container.clientWidth / container.clientHeight;
    camera.updateProjectionMatrix();
    renderer.setSize(container.clientWidth, container.clientHeight);
});
function animate(timestamp) {
    requestAnimationFrame(animate);
    if (demoMode) {
        const elapsed = timestamp - lastPresetSwitchTime;
        if (elapsed > PRESET_DURATION) {
            currentPresetIndex = (currentPresetIndex + 1) % presetKeys.length;
            const key = presetKeys[currentPresetIndex];
            applyPresetValues(presets[key], `Demo (${presets[key].name})`);
            lastPresetSwitchTime = timestamp;
        }
    }
    // Slow spatial rotation for depth perception.
    hopfGroup.rotation.x += 0.003;
    hopfGroup.rotation.y += 0.005;
    controls.update();
    renderer.render(scene, camera);
}
applyPresetValues(presets.fundamental, 'Demo (Fundamental Hopfion)');
lastPresetSwitchTime = performance.now();
requestAnimationFrame(animate);
