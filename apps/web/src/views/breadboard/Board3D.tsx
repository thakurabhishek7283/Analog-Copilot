import { useEffect, useRef, useState } from "react";
import * as THREE from "three";
import { OrbitControls } from "three/addons/controls/OrbitControls.js";
import { BOARD_ROWS, holePosition, type BreadboardLayout } from "./model.ts";

type Pick = { kind: "hole" | "part" | "wire"; id: string };
const COLUMNS = "ABCDEFGHIJ";
const WIRE_COLORS = [0xe8590c, 0x1c7ed6, 0x2f9e44, 0xae3ec9, 0xf08c00, 0x0c8599];

function label(text: string, x: number, z: number): THREE.Sprite {
  const canvas = document.createElement("canvas");
  canvas.width = 256;
  canvas.height = 64;
  const ctx = canvas.getContext("2d")!;
  ctx.fillStyle = "#ffffff";
  ctx.font = "bold 36px sans-serif";
  ctx.textAlign = "center";
  ctx.fillText(text, 128, 44);
  const texture = new THREE.CanvasTexture(canvas);
  const sprite = new THREE.Sprite(new THREE.SpriteMaterial({ map: texture, transparent: true, depthTest: false }));
  sprite.position.set(x, 0.8, z);
  sprite.scale.set(1.7, 0.42, 1);
  return sprite;
}

function wireMesh(from: string, to: string, color: number): THREE.Mesh {
  const [ax, az] = holePosition(from);
  const [bx, bz] = holePosition(to);
  const midY = 0.7 + Math.min(1.2, Math.hypot(ax - bx, az - bz) * 0.07);
  const points = [
    new THREE.Vector3(ax, 0.32, az),
    new THREE.Vector3(ax, midY, az),
    new THREE.Vector3((ax + bx) / 2, midY + 0.12, (az + bz) / 2),
    new THREE.Vector3(bx, midY, bz),
    new THREE.Vector3(bx, 0.32, bz),
  ];
  return new THREE.Mesh(new THREE.TubeGeometry(new THREE.CatmullRomCurve3(points), 24, 0.065, 6, false), new THREE.MeshStandardMaterial({ color, roughness: 0.5 }));
}

/** Procedural board; OrbitControls renders only on interaction, never in an idle animation loop. */
export function Board3D({ layout, partTypes, selected, fitSignal, onPick }: { layout: BreadboardLayout; partTypes: Record<string, string>; selected: Pick | null; fitSignal: number; onPick: (pick: Pick) => void }) {
  const host = useRef<HTMLDivElement>(null);
  const view = useRef<{ boards: number; fitSignal: number; camera: THREE.Vector3; target: THREE.Vector3 } | null>(null);
  const onPickRef = useRef(onPick);
  onPickRef.current = onPick;
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    const element = host.current;
    if (!element) return;
    const scene = new THREE.Scene();
    scene.background = new THREE.Color(0x20252b);
    let renderer: THREE.WebGLRenderer;
    try { renderer = new THREE.WebGLRenderer({ antialias: true }); }
    catch { setError("3D graphics are unavailable in this browser."); return; }
    renderer.setPixelRatio(Math.min(devicePixelRatio, 2));
    renderer.outputColorSpace = THREE.SRGBColorSpace;
    element.appendChild(renderer.domElement);
    const camera = new THREE.PerspectiveCamera(48, 1, 0.1, 500);
    const occupied = [...Object.values(layout.parts).flatMap((part) => Object.values(part.holes)), ...layout.jumpers.flatMap((wire) => [wire.from, wire.to])].map((hole) => holePosition(hole)[1]);
    const focusZ = occupied.length ? (Math.min(...occupied) + Math.max(...occupied)) / 2 : 0;
    const focusX = (layout.boards - 1) * 6.5 - 1.3;
    const saved = view.current?.boards === layout.boards && view.current.fitSignal === fitSignal ? view.current : null;
    const target = saved?.target ?? new THREE.Vector3(focusX, 0, focusZ);
    camera.position.copy(saved?.camera ?? new THREE.Vector3(focusX + 20, Math.max(55, layout.boards * 10), focusZ + Math.max(40, layout.boards * 6)));
    camera.lookAt(target);
    const controls = new OrbitControls(camera, renderer.domElement);
    controls.target.copy(target);
    controls.enableDamping = false;
    controls.minDistance = 5;
    controls.maxDistance = Math.max(75, layout.boards * 24);
    controls.maxPolarAngle = Math.PI * 0.48;
    scene.add(new THREE.HemisphereLight(0xffffff, 0x697481, 2.2));
    const light = new THREE.DirectionalLight(0xffffff, 2);
    light.position.set(-6, 22, 14);
    scene.add(light);
    for (let n = 1; n <= layout.boards; n++) {
      const dx = (n - 1) * 13;
      const board = new THREE.Mesh(new THREE.BoxGeometry(11.3, 0.65, 32), new THREE.MeshStandardMaterial({ color: 0xe5e3d5, roughness: 0.85 }));
      board.position.set(dx - 0.15, -0.34, 0);
      scene.add(board);
      const trench = new THREE.Mesh(new THREE.BoxGeometry(1.05, 0.12, 31.5), new THREE.MeshStandardMaterial({ color: 0x9e9f99 }));
      trench.position.set(dx + 0.75, 0.01, 0);
      scene.add(trench);
      scene.add(label(`Board ${n}`, dx - 0.1, -16.7));
      for (let row = 5; row <= BOARD_ROWS; row += 5) {
        const marker = label(String(row), dx - 5.7, holePosition(`B${n}:A${row}`)[1]);
        marker.position.y = 0.15;
        marker.scale.set(0.55, 0.22, 1);
        scene.add(marker);
      }
    }
    const holes: string[] = [];
    for (let n = 1; n <= layout.boards; n++) for (let row = 1; row <= BOARD_ROWS; row++) for (const column of COLUMNS) holes.push(`B${n}:${column}${row}`);
    const holeMesh = new THREE.InstancedMesh(new THREE.CylinderGeometry(0.12, 0.12, 0.015, 8), new THREE.MeshStandardMaterial({ color: 0x353a40 }), holes.length);
    const hitMesh = new THREE.InstancedMesh(new THREE.CylinderGeometry(0.28, 0.28, 0.08, 8), new THREE.MeshBasicMaterial({ transparent: true, opacity: 0, depthWrite: false }), holes.length);
    const dummy = new THREE.Object3D();
    holes.forEach((hole, index) => {
      const [x, z] = holePosition(hole);
      dummy.position.set(x, 0.025, z);
      dummy.updateMatrix();
      holeMesh.setMatrixAt(index, dummy.matrix);
      dummy.position.y = 0.09;
      dummy.updateMatrix();
      hitMesh.setMatrixAt(index, dummy.matrix);
    });
    holeMesh.instanceMatrix.needsUpdate = true;
    hitMesh.instanceMatrix.needsUpdate = true;
    scene.add(holeMesh, hitMesh);
    const pickables: THREE.Object3D[] = [hitMesh];

    for (const [ref, placed] of Object.entries(layout.parts)) {
      const positions = Object.values(placed.holes).map(holePosition);
      const x = positions.reduce((sum, point) => sum + point[0], 0) / positions.length;
      const z = positions.reduce((sum, point) => sum + point[1], 0) / positions.length;
      const external = placed.row === 0;
      const kind = partTypes[ref] ?? "";
      const spanX = Math.max(...positions.map((p) => p[0])) - Math.min(...positions.map((p) => p[0]));
      const spanZ = Math.max(...positions.map((p) => p[1])) - Math.min(...positions.map((p) => p[1]));
      const passive = /^(resistor|diode|zener|inductor)/.test(kind);
      const cylindrical = /^(cap_elec|npn_|pnp_)/.test(kind);
      const led = kind.startsWith("led_");
      const color = selected?.kind === "part" && selected.id === ref ? 0xffc857 : external ? 0x42566a : kind.startsWith("resistor") ? 0xc9aa78 : kind.startsWith("led_") ? 0xd74036 : kind.startsWith("cap_") ? 0x3c6f91 : kind.startsWith("inductor") ? 0x54936d : 0x30363e;
      const geometry = passive ? new THREE.CylinderGeometry(0.22, 0.22, Math.max(0.8, spanX - 0.45), 14)
        : cylindrical ? new THREE.CylinderGeometry(0.38, 0.38, Math.max(0.55, spanZ + 0.45), 16)
          : led ? new THREE.SphereGeometry(0.42, 16, 10)
            : new THREE.BoxGeometry(external ? 2.4 : Math.max(0.75, spanX - 0.4), external ? 0.45 : 0.6, external ? 1.3 : Math.max(0.48, spanZ + 0.35));
      const body = new THREE.Mesh(
        geometry,
        new THREE.MeshStandardMaterial({ color, roughness: 0.55, metalness: external ? 0.18 : 0.04 }),
      );
      if (passive) body.rotation.z = Math.PI / 2;
      if (cylindrical && !kind.startsWith("cap_elec")) body.rotation.x = Math.PI / 2;
      body.position.set(x, external ? 0.15 : 0.43, z);
      body.userData = { kind: "part", id: ref };
      scene.add(body, label(ref, x, z));
      pickables.push(body);
      if (!external) for (const [hx, hz] of positions) {
        const lead = new THREE.Mesh(new THREE.SphereGeometry(0.12, 8, 6), new THREE.MeshStandardMaterial({ color: 0xa8aaae, metalness: 0.75, roughness: 0.25 }));
        lead.position.set(hx, 0.15, hz);
        scene.add(lead);
      }
      if (!external && (Object.keys(placed.holes).length > 2 || /^(diode_|zener_|led_|cap_elec)/.test(kind))) {
        const entries = Object.entries(placed.holes);
        const marked = entries.find(([pin]) => pin === "K" || pin === "P" || pin === "A") ?? entries[0]!;
        const [hx, hz] = holePosition(marked[1]);
        const marker = label(marked[0] === "K" ? "K" : marked[0] === "P" || marked[0] === "A" ? "+" : "1", hx - 0.35, hz - 0.34);
        marker.position.y = 0.95;
        marker.scale.set(0.5, 0.2, 1);
        scene.add(marker);
      }
      if (external) {
        for (const hole of Object.values(placed.holes)) {
          const [hx, hz] = holePosition(hole);
          const terminal = new THREE.Mesh(new THREE.SphereGeometry(0.2), new THREE.MeshStandardMaterial({ color: hole.endsWith(".N") ? 0x27313b : 0xd94735 }));
          terminal.position.set(hx, 0.48, hz);
          terminal.userData = { kind: "hole", id: hole };
          scene.add(terminal);
          pickables.push(terminal);
        }
      }
    }

    layout.jumpers.forEach((jumper, index) => {
      const mesh = wireMesh(jumper.from, jumper.to, selected?.kind === "wire" && selected.id === jumper.id ? 0xffdc67 : WIRE_COLORS[index % WIRE_COLORS.length]!);
      mesh.userData = { kind: "wire", id: jumper.id };
      scene.add(mesh);
      pickables.push(mesh);
    });

    const draw = () => renderer.render(scene, camera);
    const resize = () => {
      const width = element.clientWidth;
      const height = element.clientHeight;
      if (!width || !height) return;
      camera.aspect = width / height;
      camera.updateProjectionMatrix();
      renderer.setSize(width, height, false);
      draw();
    };
    const observer = new ResizeObserver(resize);
    observer.observe(element);
    controls.addEventListener("change", draw);
    resize();
    const raycaster = new THREE.Raycaster();
    const pointer = new THREE.Vector2();
    let down: [number, number] | null = null;
    const onDown = (event: PointerEvent) => { down = [event.clientX, event.clientY]; };
    const onUp = (event: PointerEvent) => {
      if (!down || Math.hypot(event.clientX - down[0], event.clientY - down[1]) > 5) return;
      const rect = renderer.domElement.getBoundingClientRect();
      pointer.set(((event.clientX - rect.left) / rect.width) * 2 - 1, -((event.clientY - rect.top) / rect.height) * 2 + 1);
      raycaster.setFromCamera(pointer, camera);
      const hit = raycaster.intersectObjects(pickables, false)[0];
      if (!hit) return;
      if (hit.object === hitMesh && hit.instanceId !== undefined) onPickRef.current({ kind: "hole", id: holes[hit.instanceId]! });
      else if (hit.object.userData.kind) onPickRef.current(hit.object.userData as Pick);
    };
    renderer.domElement.addEventListener("pointerdown", onDown);
    renderer.domElement.addEventListener("pointerup", onUp);
    return () => {
      view.current = { boards: layout.boards, fitSignal, camera: camera.position.clone(), target: controls.target.clone() };
      observer.disconnect();
      controls.dispose();
      renderer.domElement.removeEventListener("pointerdown", onDown);
      renderer.domElement.removeEventListener("pointerup", onUp);
      scene.traverse((object) => {
        if (object instanceof THREE.Mesh || object instanceof THREE.InstancedMesh) {
          object.geometry.dispose();
          const materials = Array.isArray(object.material) ? object.material : [object.material];
          materials.forEach((material) => material.dispose());
        }
        if (object instanceof THREE.Sprite) { object.material.map?.dispose(); object.material.dispose(); }
      });
      renderer.dispose();
      renderer.domElement.remove();
    };
  }, [layout, partTypes, selected, fitSignal]);
  return <div className="breadboard-canvas" ref={host}>{error && <p role="alert">{error}</p>}</div>;
}
