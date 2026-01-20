if (window.__IS_TAURI__) {
    (async () => {
        const { open } = await import('@tauri-apps/plugin-shell');
        const { getCurrentWindow } = await import('@tauri-apps/api/window');
        const { save } = await import('@tauri-apps/plugin-dialog');
        const { writeTextFile } = await import('@tauri-apps/plugin-fs');

        document.documentElement.classList.add('tauri');

        const appWindow = getCurrentWindow();

        document.addEventListener('click', async (e) => {
        const link = e.target.closest('a[href^="http"]');
        if (link && !link.href.startsWith('http://127.0.0.1')) {
            e.preventDefault();
            await open(link.href);
        }
        }, true);

        // Intercept export form submissions to use native save dialog
        document.addEventListener('submit', async (e) => {
            const form = e.target;
            if (form.action && form.action.includes('/export')) {
                e.preventDefault();

                // Find the class-toggle controller container to reset UI after export
                const toggleContainer = form.closest('[data-controller="class-toggle"]');
                const resetToggle = () => {
                    if (toggleContainer) {
                        const togglables = toggleContainer.querySelectorAll('[data-class-toggle-target="togglable"]');
                        togglables.forEach(el => el.classList.toggle('hidden'));
                    }
                };

                try {
                    // Get CSRF token from meta tag
                    const csrfToken = document.querySelector('meta[name="csrf-token"]')?.content;

                    // Fetch the CSV data
                    const response = await fetch(form.action, {
                        method: (form.method || 'POST').toUpperCase(),
                        headers: {
                            'X-CSRF-Token': csrfToken,
                        },
                        credentials: 'same-origin',
                    });

                    if (!response.ok) {
                        throw new Error('Export failed');
                    }

                    const csvContent = await response.text();

                    // Reset toggle state before showing dialog
                    resetToggle();

                    // Get bot name from the page header for filename
                    const botLabelEl = document.querySelector('[id^="label_bots_"]');
                    const botName = botLabelEl?.textContent?.trim() || 'orders';
                    const filename = botName.toLowerCase().replace(/\s+/g, '-') + '.csv';

                    // Show native save dialog
                    const filePath = await save({
                        defaultPath: filename,
                        filters: [{ name: 'CSV', extensions: ['csv'] }],
                    });

                    if (filePath) {
                        await writeTextFile(filePath, csvContent);
                    }
                } catch (error) {
                    console.error('Export error:', error);
                    resetToggle();
                    alert('Failed to export: ' + error.message);
                }
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