if (window.__IS_TAURI__) {
    (async () => {
        const { open } = await import('@tauri-apps/plugin-shell');
        const { getCurrentWindow } = await import('@tauri-apps/api/window');

        document.documentElement.classList.add('tauri');

        const appWindow = getCurrentWindow();

        document.addEventListener('click', async (e) => {
        const link = e.target.closest('a[href^="http"]');
        if (link && !link.href.startsWith('http://127.0.0.1')) {
            e.preventDefault();
            await open(link.href);
        }
        }, true);

        const initDrag = () => {
        const dragRegion = document.querySelector('.titlebar-drag-region');
        if (dragRegion && !dragRegion._tauriInit) {
            dragRegion._tauriInit = true;
            dragRegion.addEventListener('mousedown', (e) => {
            if (e.buttons === 1) {
                appWindow.startDragging();
            }
            });
        }
        };

        initDrag();
        document.addEventListener('turbo:load', initDrag);
        document.addEventListener('DOMContentLoaded', initDrag);
    })();
}