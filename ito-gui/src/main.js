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
      const diff = await invoke("calculate_diff", { dir: currentDir });
      renderDiff(diff);
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
