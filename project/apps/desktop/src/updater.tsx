import { invoke } from '@tauri-apps/api/core';
import { useEffect, useState } from 'react';

export function DesktopUpdater() {
  const [enabled, setEnabled] = useState(false);
  const [version, setVersion] = useState<string | null>(null);
  const [pending, setPending] = useState<'check' | 'install' | null>(null);
  const [status, setStatus] = useState('');
  const [error, setError] = useState('');

  useEffect(() => {
    let active = true;
    void invoke<boolean>('updater_enabled').then(
      (configured) => { if (active) setEnabled(configured); },
      () => { if (active) setEnabled(false); },
    );
    return () => { active = false; };
  }, []);

  async function check() {
    setPending('check');
    setVersion(null);
    setStatus('Checking for updates…');
    setError('');
    try {
      const update = await invoke<string | null>('check_for_update');
      setVersion(update);
      setStatus(update ? `Version ${update} is available.` : 'You have the latest version.');
    } catch (err) {
      setStatus('');
      setError(String(err));
    } finally {
      setPending(null);
    }
  }

  async function install(update: string) {
    setPending('install');
    setStatus('Downloading and verifying the update…');
    setError('');
    try {
      await invoke('install_update', { version: update });
      setStatus('Update installed. Restarting…');
    } catch (err) {
      setStatus('');
      setError(String(err));
      setPending(null);
    }
  }

  if (!enabled) return null;
  return <footer aria-label="Desktop updates" style={{ fontFamily: 'system-ui', maxWidth: 700, margin: '0 auto 36px', padding: '0 16px' }}>
    <button type="button" disabled={pending !== null} onClick={() => void check()}>Check for updates</button>
    {version && <button type="button" disabled={pending !== null} onClick={() => void install(version)}>Install and restart</button>}
    {status && <p role="status">{status}</p>}
    {error && <p role="alert">{error}</p>}
  </footer>;
}
