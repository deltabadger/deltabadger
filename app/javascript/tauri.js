if (window.__IS_TAURI__) {
    (async () => {
        document.documentElement.classList.add('tauri');

        // No browser context menu (Reload, Inspect, Back) over the app's chrome. Where one can
        // type it stays: that is where Paste lives.
        document.addEventListener('contextmenu', (e) => {
            if (!e.target.closest('input, textarea, [contenteditable]')) e.preventDefault();
        });

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

        // Updates (PLAN-B7 §5): the app's update agent holds the one state; this page shows it in
        // the banner (both layouts) and the Settings row. Every call carries the launch token the
        // engine rendered, or the app refuses it. Each state has a sequence number: the newest wins.
        const { listen } = await import('@tauri-apps/api/event');
        const token = () => document.querySelector('meta[name="desktop-launch"]')?.content || '';
        const call = (command) => invoke(command, { token: token() });

        // PLAN-B6-3 §2.2: the engine's sign-in page in the window (an expired cookie, cleared storage): the app signs the
        // window in again with its own ticket. Outside the app this page is the browser's code field.
        let signInAsked = null;
        const signInWindow = () => {
            const form = document.querySelector('[data-desktop-sign-in]');
            if (form && token() && signInAsked !== form) {
                signInAsked = form;
                invoke('sign_in_window', { token: token() }).catch((error) => console.error('Sign in:', error));
            }
        };
        signInWindow();
        document.addEventListener('turbo:load', signInWindow);
        let update = null;
        let checkedHere = false;

        const lines = (s, t) => {
            const pct = (key) => s.percent == null ? t[key] : t[key + 'Percent'].replace('%{percent}', s.percent);
            switch (s.state) {
                case 'downloading': return { line: pct(s.critical ? 'textCriticalDownloading' : 'textDownloading'), critical: s.critical };
                case 'ready': return { line: s.critical ? t.textCriticalReady : t.textReady, more: t.textReadyMore, action: 'install', critical: s.critical };
                case 'failed': return { line: t['textFailed' + s.kind[0].toUpperCase() + s.kind.slice(1)], action: 'retry', link: s.kind === 'install' };
                case 'deb-notice': return { line: t.textDeb, link: true, about: t.textAboutCheckOnly };
                case 'manual': return { line: t.textManual, link: true, about: t.textAboutCheckOnly };
                default: return null;
            }
        };

        const render = () => {
            const banner = document.getElementById('desktop_update');
            if (!banner || !update) return;
            const t = banner.dataset;
            const shown = lines(update, t);
            banner.classList.toggle('hidden', !shown);
            banner.classList.toggle('salert--danger', !!shown?.critical);
            banner.querySelector('[data-desktop-update-line]').textContent = shown?.line || '';
            banner.querySelector('[data-desktop-update-more]').textContent = shown?.more || '';
            const action = banner.querySelector('[data-desktop-update-action]');
            action.classList.toggle('hidden', !shown?.action);
            action.dataset.command = shown?.action === 'install' ? 'install_update' : 'retry_update';
            action.textContent = shown?.action === 'install' ? t.textRestart : t.textTryAgain;
            banner.querySelector('[data-desktop-update-link]').classList.toggle('hidden', !shown?.link);

            const row = document.querySelector('#app_update');
            const status = row?.querySelector('[data-desktop-update-status]');
            const button = row?.querySelector('[data-desktop-update]');
            if (status) {
                status.textContent = shown?.about || shown?.line || {
                    'checking': t.textChecking,
                    'check-failed': t.textCheckFailed,
                    'idle': checkedHere ? t.textUpToDate : t.textAbout,
                }[update.state] || t.textAbout;
            }
            if (button) {
                const ready = update.state === 'ready';
                button.textContent = ready ? t.textRestart : t.textCheckNow;
                button.disabled = ['checking', 'downloading', 'stopping', 'installing'].includes(update.state);
            }
        };

        const show = (next) => {
            if (!next || (update && next.seq < update.seq)) return;
            update = next;
            render();
        };

        if (document.querySelector('meta[name="desktop-launch"]')) {
            // Subscribe first, then the snapshot: a change between the two is never lost (r1.12).
            await listen('update-state', (event) => show(event.payload));
            try { show(await call('update_state')); } catch (error) { console.error('Update state:', error); }
            document.addEventListener('turbo:load', render);
            document.addEventListener('turbo:frame-load', render);
        }

        document.addEventListener('click', async (e) => {
            const action = e.target.closest('[data-desktop-update-action]');
            const button = e.target.closest('[data-desktop-update]');
            if (!action && !button) return;
            e.preventDefault();
            try {
                if (action) {
                    show(await call(action.dataset.command));
                } else if (update?.state === 'ready') {
                    show(await call('install_update'));
                } else {
                    checkedHere = true;
                    show(await call('check_for_updates'));
                }
            } catch (error) {
                console.error('Update:', error);
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