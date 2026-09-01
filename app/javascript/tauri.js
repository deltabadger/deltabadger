if (window.__IS_TAURI__) {
    (async () => {
        document.documentElement.classList.add('tauri');

        const { open } = await import('@tauri-apps/plugin-shell');
        const { invoke } = await import('@tauri-apps/api/core');
        const { getCurrentWindow } = await import('@tauri-apps/api/window');
        const { save } = await import('@tauri-apps/plugin-dialog');
        const { writeTextFile } = await import('@tauri-apps/plugin-fs');

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

        // The Settings update button. The desktop build installs updates itself, so it gets a
        // button rather than the instructions the container platforms need. Everything after
        // finding an update is the native prompt, and a successful install restarts the app, so
        // the only outcome worth reporting here is that there was nothing to install.
        document.addEventListener('click', async (e) => {
            const button = e.target.closest('[data-desktop-update]');
            if (!button) return;
            e.preventDefault();

            const status = button.parentElement.querySelector('[data-desktop-update-status]');
            const idle = button.textContent;
            button.disabled = true;
            button.textContent = button.dataset.desktopUpdateChecking;

            try {
                const found = await invoke('check_for_updates');
                if (!found && status) status.textContent = button.dataset.desktopUpdateUpToDate;
            } catch (error) {
                console.error('Update check failed:', error);
                if (status) status.textContent = button.dataset.desktopUpdateFailed;
            } finally {
                button.disabled = false;
                button.textContent = idle;
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