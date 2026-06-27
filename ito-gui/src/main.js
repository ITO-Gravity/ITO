const { invoke } = window.__TAURI__.core;

// Elementos de la UI
let btnSelectFolder;
let selectedPathEl;
let infoProjectIdEl;
let infoRemoteUrlEl;
let statusDesignEl;
let statusBomEl;
let btnRunDiff;
let diffResultsEl;
let pushMessageEl;
let btnPush;
let pushLogEl;
let historyListEl;

let currentDir = "";

// Cambio de Pestañas (Tabs)
function setupTabs() {
  const tabDiff = document.getElementById("tab-diff");
  const tabHistory = document.getElementById("tab-history");
  const viewDiff = document.getElementById("view-diff");
  const viewHistory = document.getElementById("view-history");

  tabDiff.addEventListener("click", () => {
    tabDiff.classList.add("active");
    tabHistory.classList.remove("active");
    viewDiff.classList.add("active");
    viewHistory.classList.remove("active");
  });

  tabHistory.addEventListener("click", () => {
    tabHistory.classList.add("active");
    tabDiff.classList.remove("active");
    viewHistory.classList.add("active");
    viewDiff.classList.remove("active");
    if (currentDir) {
      refreshProjectStatus();
    }
  });
}

// Cargar y refrescar información del proyecto
async function refreshProjectStatus() {
  if (!currentDir) return;

  try {
    const status = await invoke("load_project_status", { dir: currentDir });
    
    // Actualizar datos de info en el sidebar
    infoProjectIdEl.textContent = status.project_id;
    infoRemoteUrlEl.textContent = status.remote_url || "-";
    infoRemoteUrlEl.title = status.remote_url || "-";

    // Actualizar estados visuales de archivos
    statusDesignEl.innerHTML = status.design_exists 
      ? '<span style="color: #10b981;">🟢 Detectado</span>' 
      : '<span style="color: #ef4444;">🔴 Ausente</span>';
    statusBomEl.innerHTML = status.bom_exists 
      ? '<span style="color: #10b981;">🟢 Detectado</span>' 
      : '<span style="color: #ef4444;">🔴 Ausente</span>';

    // Habilitar/Deshabilitar botones
    btnRunDiff.disabled = !status.design_exists;
    pushMessageEl.disabled = !status.design_exists;
    btnPush.disabled = !status.design_exists;

    // Renderizar Historial
    renderHistory(status.history);

  } catch (error) {
    showPushLog("Error al cargar estado del proyecto: " + error, true);
  }
}

// Renderizar Historial Local
function renderHistory(commits) {
  if (!commits || commits.length === 0) {
    historyListEl.innerHTML = `
      <div class="empty-state">
        <span class="empty-icon">📜</span>
        <h3>Historial vacío</h3>
        <p>No se encontraron commits locales para este proyecto.</p>
      </div>`;
    return;
  }

  historyListEl.innerHTML = commits.map(commit => {
    const truncatedHash = commit.hash.substring(0, 7);
    const syncBadge = commit.synced 
      ? '<span class="history-badge badge-synced">Sincronizado</span>'
      : '<span class="history-badge badge-pending">Pendiente de Subida</span>';

    return `
      <div class="history-card">
        <div class="history-info">
          <div class="history-header">
            <span class="history-hash">${truncatedHash}</span>
            <span class="history-date">${commit.timestamp}</span>
          </div>
          <div class="history-msg">${commit.message}</div>
        </div>
        <div>
          ${syncBadge}
        </div>
      </div>`;
  }).reverse().join("");
}

// Formatear cambios del diff
function renderDiff(diff) {
  let html = "";
  let hasChanges = false;

  // 1. Componentes Añadidos
  const compAdded = Object.keys(diff.components.added);
  const compDeleted = Object.keys(diff.components.deleted);
  const compModified = Object.keys(diff.components.modified);

  if (compAdded.length > 0 || compDeleted.length > 0 || compModified.length > 0) {
    hasChanges = true;
    html += `<div class="diff-section">
      <div class="diff-section-title">📦 Componentes</div>`;

    compAdded.forEach(des => {
      const comp = diff.components.added[des];
      html += `
        <div class="diff-card">
          <div class="diff-card-header">
            <span class="diff-card-title">${des}</span>
            <span class="diff-card-badge badge-added">Añadido</span>
          </div>
          <div class="diff-change-line"><span class="diff-change-label">MPN:</span> ${comp.mpn || "No especificado"}</div>
          <div class="diff-change-line"><span class="diff-change-label">Footprint:</span> ${comp.footprint || "No especificado"}</div>
        </div>`;
    });

    compDeleted.forEach(des => {
      const comp = diff.components.deleted[des];
      html += `
        <div class="diff-card">
          <div class="diff-card-header">
            <span class="diff-card-title">${des}</span>
            <span class="diff-card-badge badge-deleted">Eliminado</span>
          </div>
          <div class="diff-change-line"><span class="diff-change-label">MPN:</span> ${comp.mpn || "No especificado"}</div>
        </div>`;
    });

    compModified.forEach(des => {
      const compMod = diff.components.modified[des];
      html += `
        <div class="diff-card">
          <div class="diff-card-header">
            <span class="diff-card-title">${des}</span>
            <span class="diff-card-badge badge-modified">Modificado</span>
          </div>`;
      
      if (compMod.footprint_change) {
        html += `<div class="diff-change-line"><span class="diff-change-label">Huella:</span> ${compMod.footprint_change.old} ➔ ${compMod.footprint_change.new}</div>`;
      }
      if (compMod.value_change) {
        html += `<div class="diff-change-line"><span class="diff-change-label">Valor:</span> ${compMod.value_change.old} ➔ ${compMod.value_change.new}</div>`;
      }
      if (compMod.mpn_change) {
        html += `<div class="diff-change-line"><span class="diff-change-label">MPN:</span> ${compMod.mpn_change.old} ➔ ${compMod.mpn_change.new}</div>`;
      }
      
      html += `</div>`;
    });

    html += `</div>`;
  }

  // 2. Redes / Conexiones
  const netsAdded = Object.keys(diff.nets.added);
  const netsDeleted = Object.keys(diff.nets.deleted);
  const netsModified = Object.keys(diff.nets.modified);

  if (netsAdded.length > 0 || netsDeleted.length > 0 || netsModified.length > 0) {
    hasChanges = true;
    html += `<div class="diff-section">
      <div class="diff-section-title">⚡ Redes Eléctricas (Nets)</div>`;

    netsAdded.forEach(name => {
      html += `
        <div class="diff-card">
          <div class="diff-card-header">
            <span class="diff-card-title">${name}</span>
            <span class="diff-card-badge badge-added">Red Añadida</span>
          </div>
        </div>`;
    });

    netsDeleted.forEach(name => {
      html += `
        <div class="diff-card">
          <div class="diff-card-header">
            <span class="diff-card-title">${name}</span>
            <span class="diff-card-badge badge-deleted">Red Eliminada</span>
          </div>
        </div>`;
    });

    netsModified.forEach(name => {
      const netMod = diff.nets.modified[name];
      html += `
        <div class="diff-card">
          <div class="diff-card-header">
            <span class="diff-card-title">${name}</span>
            <span class="diff-card-badge badge-modified">Red Modificada</span>
          </div>`;
      
      netMod.added_endpoints.forEach(ep => {
        html += `<div class="diff-change-line" style="color: #10b981;">➕ Endpoint conectado: ${ep.component_designator}:${ep.pin_id}</div>`;
      });

      netMod.deleted_endpoints.forEach(ep => {
        html += `<div class="diff-change-line" style="color: #ef4444;">➖ Endpoint desconectado: ${ep.component_designator}:${ep.pin_id}</div>`;
      });

      html += `</div>`;
    });

    html += `</div>`;
  }

  if (!hasChanges) {
    diffResultsEl.innerHTML = `
      <div class="empty-state">
        <span class="empty-icon" style="color: #10b981;">🟢</span>
        <h3>Diseño idéntico</h3>
        <p>Los diseños son semánticamente idénticos al último respaldo. No se detectaron cambios.</p>
      </div>`;
  } else {
    diffResultsEl.innerHTML = html;
  }
}

// Log de Sincronización
function showPushLog(message, isError = false) {
  pushLogEl.style.display = "block";
  pushLogEl.style.color = isError ? "#ef4444" : "#10b981";
  pushLogEl.textContent = message;
}

// Event Listeners y Carga inicial
window.addEventListener("DOMContentLoaded", () => {
  btnSelectFolder = document.getElementById("btn-select-folder");
  selectedPathEl = document.getElementById("selected-path");
  infoProjectIdEl = document.getElementById("info-project-id");
  infoRemoteUrlEl = document.getElementById("info-remote-url");
  statusDesignEl = document.getElementById("status-design");
  statusBomEl = document.getElementById("status-bom");
  btnRunDiff = document.getElementById("btn-run-diff");
  diffResultsEl = document.getElementById("diff-results");
  pushMessageEl = document.getElementById("push-message");
  btnPush = document.getElementById("btn-push");
  pushLogEl = document.getElementById("push-log");
  historyListEl = document.getElementById("history-list");

  setupTabs();

  // 1. Selector de Carpeta
  btnSelectFolder.addEventListener("click", async () => {
    const selected = await invoke("select_folder");
    if (selected) {
      currentDir = selected;
      selectedPathEl.textContent = selected;
      selectedPathEl.title = selected;
      
      // Limpiar logs y diffs previos
      diffResultsEl.innerHTML = `
        <div class="empty-state">
          <span class="empty-icon">🔍</span>
          <h3>Sin diferencias calculadas</h3>
          <p>Haz click en "Calcular Cambios" para contrastar el diseño.</p>
        </div>`;
      document.getElementById("pcb-canvas-container").innerHTML = `
        <div class="empty-state">
          <span class="empty-icon">🖥️</span>
          <h3>PCB sin renderizar</h3>
          <p>Calcula los cambios para ver el renderizado interactivo antes/después del hardware.</p>
        </div>`;
      pushLogEl.style.display = "none";
      pushMessageEl.value = "";

      refreshProjectStatus();
    }
  });

  // 2. Ejecutar Diff
  btnRunDiff.addEventListener("click", async () => {
    if (!currentDir) return;
    btnRunDiff.disabled = true;
    btnRunDiff.textContent = "⌛ Analizando...";
    
    try {
      const payload = await invoke("calculate_diff", { dir: currentDir });
      renderDiff(payload.diff);
      renderPcb(payload.old_design, payload.new_design, payload.diff);
    } catch (error) {
      diffResultsEl.innerHTML = `
        <div class="empty-state">
          <span class="empty-icon">🔴</span>
          <h3>Error en el Análisis</h3>
          <p>${error}</p>
        </div>`;
    } finally {
      btnRunDiff.disabled = false;
      btnRunDiff.textContent = "⚡ Calcular Cambios";
    }
  });

  // 3. Ejecutar Push / Backup
  btnPush.addEventListener("click", async () => {
    if (!currentDir) return;

    const message = pushMessageEl.value.trim() || null;
    btnPush.disabled = true;
    btnPush.textContent = "⌛ Procesando...";
    pushLogEl.style.display = "none";

    try {
      const result = await invoke("push_project", { dir: currentDir, message });
      
      if (result.success) {
        showPushLog(result.message, false);
        pushMessageEl.value = "";
        
        // Recargar diff y estado
        refreshProjectStatus();
        
        // Resetear visualización de diff
        diffResultsEl.innerHTML = `
          <div class="empty-state">
            <span class="empty-icon" style="color: #10b981;">🟢</span>
            <h3>Proyecto Respaldado</h3>
            <p>Se ha generado el punto de control local con éxito.</p>
          </div>`;
      } else {
        showPushLog(result.message, true);
        refreshProjectStatus();
      }
    } catch (error) {
      showPushLog("Error al realizar push: " + error, true);
    } finally {
      btnPush.disabled = false;
      btnPush.textContent = "🚀 Respaldar y Subir (Push)";
    }
  });
});

function getComponentPos(designator) {
  const type = designator.charAt(0);
  const num = parseInt(designator.substring(1)) || 1;
  let x = 100, y = 100;
  if (type === 'U') {
    x = 150 + (num * 100);
    y = 200;
  } else if (type === 'R') {
    x = 420;
    y = 80 + (num * 65);
  } else if (type === 'C') {
    x = 540;
    y = 80 + (num * 65);
  } else {
    x = 250;
    y = 330 + (num * 50);
  }
  return { x, y };
}

function getPinOffset(pinId, compWidth = 40, compHeight = 40) {
  const pin = parseInt(pinId) || 1;
  if (pin % 2 === 1) {
    return { x: -compWidth/2, y: 0 };
  } else {
    return { x: compWidth/2, y: 0 };
  }
}

function renderPinsSvg(designator, x, y, w, h) {
  const type = designator.charAt(0);
  let pinsHtml = "";
  if (type === 'U') {
    for (let i = 0; i < 4; i++) {
      pinsHtml += `<rect x="${x - w/2 - 6}" y="${y - h/2 + 8 + i*14}" width="6" height="4" class="component-pin" />`;
      pinsHtml += `<rect x="${x + w/2}" y="${y - h/2 + 8 + i*14}" width="6" height="4" class="component-pin" />`;
    }
  } else {
    pinsHtml += `<rect x="${x - w/2 - 4}" y="${y - 2}" width="4" height="4" class="component-pin" />`;
    pinsHtml += `<rect x="${x + w/2}" y="${y - 2}" width="4" height="4" class="component-pin" />`;
  }
  return pinsHtml;
}

function renderPcb(old_design, new_design, diff) {
  const components = {};
  
  if (old_design && old_design.components) {
    Object.keys(old_design.components).forEach(des => {
      components[des] = {
        designator: des,
        status: 'deleted',
        details: old_design.components[des]
      };
    });
  }
  
  if (new_design && new_design.components) {
    Object.keys(new_design.components).forEach(des => {
      if (components[des]) {
        const isModified = diff.components.modified[des] !== undefined;
        components[des].status = isModified ? 'modified' : 'normal';
        components[des].details = new_design.components[des];
      } else {
        components[des] = {
          designator: des,
          status: 'added',
          details: new_design.components[des]
        };
      }
    });
  }

  let svgContent = `
    <svg class="pcb-svg" viewBox="0 0 650 450" xmlns="http://www.w3.org/2000/svg">
      <defs>
        <pattern id="grid-pattern" width="20" height="20" patternUnits="userSpaceOnUse">
          <path d="M 20 0 L 0 0 0 20" fill="none" stroke="rgba(255,255,255,0.015)" stroke-width="1" />
        </pattern>
      </defs>
      
      <rect x="10" y="10" width="630" height="430" class="pcb-board" />
      <rect x="10" y="10" width="630" height="430" fill="url(#grid-pattern)" rx="15" />
  `;

  // Dibujar Pistas (Nets)
  const allNets = new Set([
    ...Object.keys((old_design && old_design.nets) || {}),
    ...Object.keys(new_design.nets || {})
  ]);

  allNets.forEach(netName => {
    const isAdded = diff.nets.added[netName] !== undefined;
    const isDeleted = diff.nets.deleted[netName] !== undefined;
    
    const netDetails = new_design.nets[netName] || (old_design && old_design.nets[netName]);
    if (!netDetails) return;

    const endpoints = netDetails.endpoints || [];
    if (endpoints.length > 1) {
      for (let i = 0; i < endpoints.length - 1; i++) {
        const des1 = endpoints[i].component_designator;
        const des2 = endpoints[i+1].component_designator;
        
        const p1 = getComponentPos(des1);
        const p2 = getComponentPos(des2);
        
        // Component shape sizes for pin offset
        let w1 = 40, h1 = 40;
        if (des1.charAt(0) === 'U') { w1 = 60; h1 = 60; }
        else if (des1.charAt(0) === 'R') { w1 = 30; h1 = 18; }
        else if (des1.charAt(0) === 'C') { w1 = 24; h1 = 24; }

        let w2 = 40, h2 = 40;
        if (des2.charAt(0) === 'U') { w2 = 60; h2 = 60; }
        else if (des2.charAt(0) === 'R') { w2 = 30; h2 = 18; }
        else if (des2.charAt(0) === 'C') { w2 = 24; h2 = 24; }

        const offset1 = getPinOffset(endpoints[i].pin_id, w1, h1);
        const offset2 = getPinOffset(endpoints[i+1].pin_id, w2, h2);
        
        const x1 = p1.x + offset1.x;
        const y1 = p1.y + offset1.y;
        const x2 = p2.x + offset2.x;
        const y2 = p2.y + offset2.y;

        const dx = x2 - x1;
        const dy = y2 - y1;
        
        // Draw track with a nice 45-degree angle in the center
        if (Math.abs(dx) > 30 && Math.abs(dy) > 30) {
          const midX = x1 + dx / 2;
          svgContent += `<path d="M ${x1} ${y1} L ${midX} ${y1} L ${x2} ${y2}" class="pcb-trace ${isAdded ? 'added' : (isDeleted ? 'deleted' : '')}" />`;
        } else {
          svgContent += `<line x1="${x1}" y1="${y1}" x2="${x2}" y2="${y2}" class="pcb-trace ${isAdded ? 'added' : (isDeleted ? 'deleted' : '')}" />`;
        }
      }
    }
  });

  // Dibujar Componentes
  Object.keys(components).forEach(des => {
    const comp = components[des];
    const { x, y } = getComponentPos(des);
    
    let w = 40, h = 40;
    const type = des.charAt(0);
    if (type === 'U') { w = 60; h = 60; }
    else if (type === 'R') { w = 30; h = 18; }
    else if (type === 'C') { w = 24; h = 24; }

    const rx = x - w/2;
    const ry = y - h/2;

    svgContent += `
      <g class="pcb-component" id="comp-${des}">
        <rect x="${rx}" y="${ry}" width="${w}" height="${h}" class="component-body ${comp.status}" />
        ${renderPinsSvg(des, x, y, w, h)}
        <text x="${x}" y="${y}" class="component-text">${des}</text>
        <title>${des}: ${comp.details.mpn || "Genérico"}\nEstado: ${comp.status.toUpperCase()}</title>
      </g>
    `;
  });

  svgContent += `</svg>`;
  document.getElementById("pcb-canvas-container").innerHTML = svgContent;
}
