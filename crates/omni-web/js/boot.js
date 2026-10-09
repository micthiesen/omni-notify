// Starts the wasm bundle from an external, content-hashed module so pages
// served under a strict CSP need only 'self' and 'wasm-unsafe-eval' (no
// inline script). Trunk writes the bundle paths into <meta name="omni-bundle">.
const bundle = document.querySelector('meta[name="omni-bundle"]');
if (bundle) {
  const { default: init } = await import(bundle.dataset.js);
  await init({ module_or_path: bundle.dataset.wasm });
}
