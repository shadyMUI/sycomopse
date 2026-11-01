# Ripresa dei Pacchetti Nativi Autonomi

## Punto di Ripresa

- Sessione: `Implementazione OpenRazer e System Tray in Tauri`, `ses_e6c10f187ffeg20Go7soDCdzUK`.
- Ultimo lavoro richiesto: pacchetti autonomi che non richiedano `install.sh`, compatibilita HeadsetControl e collaudo finale. La sessione si e fermata dopo aver letto i file necessari a correggere quattro findings, senza applicare le correzioni.
- Baseline attuale: commit `ff08470` e modifiche gia presenti nel worktree. DPI manuali, rimappatura, agente e tray sono gia implementati: non rifarli.
- HeadsetControl e gia allineato all'API reale `1.6`; la sessione precedente riporta 57 test Rust passati. Non sono risultati rieseguiti in questa pianificazione.
- Le quattro criticita sono ancora riscontrabili in `sources.py`, `native.py` e `arch/PKGBUILD`. `git diff --check` non segnala errori, soltanto avvisi LF/CRLF.

## Obiettivo e Confini

Completare le quattro correzioni e verificare il packaging autonomo gia iniziato. CachyOS/Arch usa `.pkg.tar.zst`, Ubuntu/Debian `.deb`, Fedora/openSUSE RPM specifici per target.

- Preservare tutte le modifiche esistenti, incluse quelle a HeadsetControl, ratbag, recovery e documentazione. Nessun reset, commit, push, pubblicazione o incremento di versione.
- Non aggiungere funzionalita mouse, nuovi modelli hardware o ulteriori distro.
- GUI/agente non-root; privilegi solo negli hook nativi del gestore pacchetti. Mai chiamare apt/pacman/dnf/zypper dagli hook, ne gestire servizi di altri utenti come root.
- Conservare maschere, preset e disabilitazioni amministrative. Toccare solo servizi e file propri dell'app.
- File principali: `packaging/sources.py`, `packaging/native.py`, `packaging/arch/PKGBUILD`, `packaging/tests/test_native.py`, `packaging/tests/test_packaging.py`; hook DEB/RPM/Arch e smoke solo dove necessario all'integrazione. README esistente solo se cambia il comportamento documentato.

## Interventi Ordinati

1. **Correggere lo staging della licenza HeadsetControl.** In `headset_candidate()` usare il file upstream `license` minuscolo del tag congelato 4.2.0, mantenendo `LICENSE` come nome nel payload documentale. Non introdurre fallback per versioni non richieste e non modificare i lock/checksum. Allineare anche la diagnostica residua che menziona API 1.8/1.9 all'API verificata 1.6. Aggiungere una regressione che controlli il nome sorgente esatto, anche su Windows case-insensitive; su Linux verificare lo staging reale.

2. **Distinguere reinstallazione e upgrade nel lifecycle.** `postinst configure` puo ricevere una versione precedente anche dopo `remove`; non basta usarla come prova di upgrade in-place. Registrare nello stato nativo schema 1 la rimozione eseguita dall'app e lo stato effettivo enabled/active prima del suo disable/stop. Alla reinstallazione ripristinare solo l'abilitazione e l'avvio precedentemente rimossi dal pacchetto; non riattivare un servizio che era gia disabilitato o fermato dall'amministratore. Una nuova configurazione riuscita consuma il marker di rimozione. Conservare il trattamento offline/pending e i helper Debian che rispettano policy-rc.d.

3. **Proteggere il primo upgrade da pacchetti precedenti.** Con `--upgrade` e senza `native-status.json`, non considerare l'assenza del registro come installazione nuova: osservare lo stato corrente senza chiamare enable/preset/start per un servizio disabilitato o fermato. Un servizio gia abilitato e attivo puo essere riavviato secondo la policy esistente. Schema sconosciuto resta conservato e non autorizza riattivazioni. Nessuna migrazione dei profili utente o modifica del loro schema.

4. **Preservare l'autostart su Arch/CachyOS.** Dichiarare in `PKGBUILD` `backup=('etc/xdg/autostart/io.peripheralhub.RemappingAgent.desktop')`, senza slash iniziale. Verificare inclusione del file nel payload e conservazione di modifiche come `Hidden=true` durante upgrade/reinstall. Mantenere conffile DEB e `%config(noreplace)` RPM gia presenti.

5. **Chiudere integrazione e verifiche.** Estendere i test esistenti prima di eseguire le suite. Mantenere lo smoke nativo che installa il solo pacchetto, senza `install.sh` o attivazioni manuali usate per nascondere un difetto. Non indebolire il gate `native_smoke` per produrre un falso risultato positivo.

## Flusso e Casi di Errore

Il gestore risolve le dipendenze e installa il payload; l'hook chiama `native.py`; il lifecycle osserva servizio/registro, applica la sola transizione autorizzata e salva il risultato. L'entry XDG avvia l'agente come utente grafico. Il registro distingue intento di rimozione e stato operativo, non sostituisce l'osservazione di systemd.

- Installazione nuova: backend abilitato/avviato quando consentito; mask, preset contrario o policy-rc.d producono uno stato esplicito non-running.
- Remove/reinstall DEB con versione precedente: il backend prima attivo torna operativo; un backend gia disabilitato/fermato resta tale.
- Upgrade senza registro: mai enable/start involontario; stato amministrativo conservato su tutti e tre i formati.
- Offline/chroot: configurazione per il boot, nessuna dichiarazione running; errori di staging licenza o avvio non diventano successo completo.
- Arresto o salvataggio fallito durante remove: riportare l'errore e conservare informazioni sufficienti per una riparazione, senza toccare servizi esterni.
- Vecchi registri schema 1 senza marker restano leggibili; solo un marker esplicito della rimozione permette il ripristino. Incertezza su dati preesistenti significa comportamento conservativo, non riattivazione presunta.

## Validazione

- Aggiungere test per licenza case-sensitive, configure/remove/configure con versione precedente, amministratore disabled/stopped, upgrade senza registro, mask/preset/policy-rc.d, offline, stato sconosciuto e metadati Arch backup.
- Eseguire `python -B -m unittest discover -s packaging/tests -p "test_*.py" -v` e `python -B -m unittest discover -s packaging/remapper/tests -p "test_*.py"`; separare eventuali skip Linux dai test passati.
- Verificare sintassi shell degli hook e di PKGBUILD con Git Bash gia disponibile, piu `git diff --check`.
- Rieseguire `npm test` e `npm run build`; test Rust desktop e `--no-default-features --locked`, formattazione e Clippy. Riutilizzare la toolchain GNU/configurazione temporanea gia impiegata, verificandola prima; nessuna installazione implicita di nuovi SDK o servizi.
- Se disponibile un ambiente Linux gia configurato, eseguire build/staging e smoke systemd della matrice, prioritariamente Arch/CachyOS e Debian/Ubuntu: installazione, upgrade, remove/reinstall, autostart modificato e rispetto dello stato amministrativo. Usare gli script CI esistenti, non transazioni native sull'host Windows.
- Se Linux non e disponibile, completare correzioni e test portabili, ma dichiarare build ELF/pacchetti, systemd reale, login/reboot, DBus/hardware, DKMS e audio ancora non verificati. Non installare Docker/WSL, non avviare CI su codice remoto diverso dal worktree e non dichiarare i target certificati.

## Consegna

Riportare le quattro correzioni, le suite realmente eseguite e i blocchi Linux residui. Non presentare l'app come pronta alla distribuzione finche build e smoke dei rispettivi pacchetti non sono passati. Non ci sono decisioni utente bloccanti: la ripresa riguarda esclusivamente il lavoro gia avviato, senza nuovi requisiti.
