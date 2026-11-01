# Controlli Mouse e Installer Linux Completi

Stato: pronto per implementazione. Nuovo intervento sopra il baseline `22fa21e`; OpenRazer autostart e close-to-tray sono gia implementati e non vanno rifatti.

## Obiettivo e Decisioni

- Campo DPI manuale sempre visibile, sincronizzato con i controlli esistenti e validato contro le capabilities reali.
- Rimappatura software dei pulsanti mouse rilevati: clic, tasto, multimedia e scorciatoie. Sequenze temporizzate, ripetizioni, comandi shell, rimappatura di tastiere autonome ed editor di profili multipli sono fuori scope.
- L'utente ha scelto rimappature attive anche dopo Esci e riattivazione automatica dopo login/riavvio/hotplug. Serve quindi un agente per utente distinto dalla GUI.
- Installazione completa per Arch/CachyOS, Ubuntu, Debian, Fedora e openSUSE con systemd. NixOS, Alpine, Void, distro immutabili, altri init system e ARM sono fuori scope in questa versione.
- Primi artefatti x86_64: Arch/CachyOS rolling, Ubuntu LTS con Python >=3.10 e WebKitGTK 4.1, Debian 12/13, Fedora stabile mantenuta, openSUSE Leap >=16 e Tumbleweed. Congelare VERSION_ID e immagini effettivamente testate nel manifest di build; non dichiarare supportate versioni non verificate o EOL.
- Installer online: backend specifici inclusi nel pacchetto dell'app; librerie native, driver e kernel headers risolti automaticamente dai gestori della distro. Offline completo per kernel arbitrari non richiesto e fuori scope.
- Autorizzazione amministrativa solo nell'installazione/riparazione esplicitamente avviata. GUI e agente restano non-root; nessuna elevazione automatica al primo avvio dell'app.
- Preservare audio, DPI/polling, single-instance, demo isolata, permessi IPC minimi e modifiche baseline estranee, incluse quelle HeadsetControl. Non inventare supporto hardware o correggere quei filtri in questo task.

## Fatti da Rispettare

- `src/components/Controls.tsx:55-109` mostra il campo numerico solo nel fallback; `MousePanel.tsx` ha gia bozza, assi e Applica. `src/api.ts` e Rust validano gia DPI discreti/range/step.
- `src/types.ts`, `models.rs` e IPC non espongono rimappatura. Il gate hardwareAvailable attuale non deve disabilitare una rimappatura software quando il mouse non ha DPI scrivibili.
- `devices.rs::usb_device` esclude attualmente i vendor generici: il backend input dovra rendere visibili anche quei mouse, senza fabbricare DPI, polling o batteria.
- Il bundle attuale genera DEB/RPM/AppImage e dichiara soltanto alcune dipendenze audio. Non esistono installer completi o PKGBUILD.
- Input Remapper stabile 2.2.1 non espone get_groups/get_running_preset del ramo attuale. L'Injector stabile inoltre indovina un altro origin_hash se quello salvato non esiste e puo risultare RUNNING dopo grab parziale. Questi comportamenti non sono accettabili per i preset gestiti dall'app.
- L'autoload standard usa anche nomi di gruppo/modello e non dimostra identita fisica: non usarlo per l'automatismo richiesto dall'utente.
- MIT dell'app gia presente in LICENSE, Cargo.toml e package.json. Input Remapper e HeadsetControl sono GPL: componenti/processi separati, attribuzioni e sorgenti corrispondenti obbligatori, senza cambiare arbitrariamente licenza dell'app.

## Architettura Scelta

1. **GUI Tauri:** prepara bozze, valida, invoca IPC, mostra solo stato osservato. Non apre /dev/input e non avvia programmi root.
2. **Core Rust condiviso:** identita mouse, capabilities, storage versionato, compilazione di preset e client DBus. Nessuna dipendenza da finestre nel core.
3. **Agente per utente:** binario `peripheral-hub-remapping-agent`, systemd user unit omonima, senza GUI. Gestisce applicazione e riattivazione dei profili desiderati; continua dopo Esci. Singleton e lock interprocesso impediscono concorrenza con la GUI.
4. **Backend di sistema privato:** engine Input Remapper 2.2.1 congelato e bridge GPL versionato, installati sotto `/usr/lib/peripheral-hub/` e gestiti da `peripheral-hub-input-remapper.service`. Usare bus `io.peripheralhub.InputRemapper`, oggetto `/io/peripheralhub/InputRemapper`, interfaccia `io.peripheralhub.InputRemapper1`. Non sostituire il pacchetto, il bus, la configurazione o i servizi Input Remapper eventualmente gia installati.

Il backend privato elimina la dipendenza AUR e la necessaria modifica di daemon condivisi su tutte le distro. Il kernel grab rimane esclusivo: un altro remapper attivo provoca Busy/NO_GRAB, mai stop o takeover automatico del servizio esterno.

## Esecuzione Ordinata

### 1. Contratti e Storage

- Aggiungere modelli Rust/TypeScript per pulsanti, sorgenti input, azioni, identita persistente, stato desiderato e stato osservato. Capabilities rimappatura separate dalle capabilities DPI/polling e dal provider hardware principale.
- Una configurazione persistente per identita mouse, schema v1, in un file dedicato XDG dell'app, separato dai profili audio. Campi essenziali: UUID, fingerprint/forza identita, bindings, enabled, autoApply, pausedForSafety e revisione.
- Riutilizzare limiti e scritture atomiche di profiles.rs; aggiungere locking tra processi, ownership e permessi privati, rifiuto traversal/symlink, dimensioni e codici bounded. Schema futuro sconosciuto -> diagnostica e nessuna attivazione automatica; nessuna migrazione dati audio.
- Rappresentare azioni come enum e codici validati, non espressioni libere. Per scorciatoie consentire modificatori e un solo tasto principale, senza DSL arbitrario.

### 2. DPI Manuali

- Aggiornare Controls.tsx e MousePanel.tsx: input numerico sempre presente accanto allo slider/select, anche nei controlli compatti. Preservare stile e accessibilita; layout adattabile al mobile.
- Bozza testuale distinta dal numero confermato: svuotamento e riscrittura validi come operazioni UI. Campo vuoto/invalido disabilita solo Applica DPI, senza inviare zero/NaN o applicare il vecchio valore.
- Sincronizzare input/slider, X/Y collegati o separati, reset, cambio dispositivo e readback. Nessun invio durante digitazione; nessun arrotondamento silenzioso. Usare validazione esistente min/max/step/discrete e mantenere set_dpi invariato.
- DPI non scrivibili restano sola lettura; la rimappatura puo comunque essere disponibile. Non accoppiare invalidita DPI e Applica polling.

### 3. Engine e Bridge Privati

- Congelare Input Remapper a commit `e9a87d13480c3b1dee654b296578a8f4e2cd31d6` (tag 2.2.1), con checksum di sorgente e patch, packaging nativo e sorgenti GPL corrispondenti. Dipendenze Python/GTK fornite dai pacchetti distro; niente pip di sistema o installer Python via rete al runtime. Verificare e costruire le sorgenti nella pipeline, non sull'host utente come root; i download del gestore pacchetti restano verificati normalmente.
- Aggiungere un bridge ristretto sopra l'engine, senza esporre le operazioni legacy stop_all/quit o DSL arbitrario sul nuovo bus. Protocollo v1: versione; inventory dei mouse/nodi reali; snapshot stato/preset/identita; apply con identita, epoch e preset atteso; stop selettivo con preset atteso.
- Inventory deve fornire capabilities EV_KEY, origin_hash calcolati dall'engine, bustype/VID/PID/uniq/phys e nodi/sysfs. Non leggere o pubblicare pressioni di tastiera. Escludere touchpad/keyboard-only e tutti i device virtuali/forwarded; per mouse compositi esporre solo gli input appartenenti alla stessa identita fisica verificata.
- Metodo apply atomico lato daemon: aggiornare discovery, confrontare fingerprint ed epoch, controllare preset corrente atteso, leggere una sola volta il preset validato, quindi avviare. Stop usa lo stesso controllo expected-preset. Evitare check-then-write non atomici.
- Verificare UID reale del chiamante tramite credenziali DBus, non un UID ricevuto dal frontend. Apply solo per sessione grafica locale attiva autorizzata; cleanup consentito al proprietario anche durante logout. Config/preset devono appartenere a quell'UID e avere formato/percorsi/codici consentiti.
- Preset immutabili con UUID e hash del payload. Sorgenti devono contenere origin_hash verificati. Modalita managed strict: vietare il fallback dell'Injector verso un altro nodo e fallire su qualunque grab mancante, rilasciando quelli parziali. RUNNING non basta se parte della configurazione non e applicabile.
- Stato confermato include identita effettiva, preset/hash e stato injector. Associare gli snapshot alla stessa generazione del daemon. Limiti output, timeout, no debug di eventi sensibili e release corretto dei tasti alla fermata.

### 4. Client, Discovery e UI Pulsanti

- Implementare il client zbus nel core con verifica protocollo/owner/epoch e errori AppError espliciti. Non ricadere sulla CLI input-remapper-control, che puo far partire pkexec.
- Associare a OpenRazer/ratbag/USB soltanto tramite seriale o percorso fisico verificato e univoco. Aggiungere veri mouse generici da inventory come Device Mouse con sole capabilities software; non confondere receiver e mouse o duplicare dispositivi gia associati.
- Salvare sorgenti logiche e ricompilare gli origin_hash dopo hotplug: /dev/input/eventN non e un'identita persistente. Seriali unici preferiti; per mouse senza seriale indicare associazione per modello/porta e richiedere consenso esplicito per autoApply su identita debole. Ambiguita -> nessuna scrittura/riattivazione.
- Nuova sezione Pulsanti: elenco rilevato, azione predefinita/clic/tasto/multimedia/scorciatoia, bozza, Applica, Ripristina originali e stato automatico. Acquisire scorciatoie solo in un controllo focalizzato, con annullamento; mai key listener globale persistente.
- Nuovi IPC gestiscono configurazione, disattivazione e stato; il backend risolve l'identita e le capabilities, non accetta un path o codice non verificato dal frontend. Risposte stale non sovrascrivono lo stato corrente.
- Verificare almeno un percorso di clic sinistro dopo le modifiche; non permettere di disabilitare tutte le vie al clic principale. Predefinito significa passthrough, non una scrittura superflua.

### 5. Agente e Lifecycle

- Aggiungere binario non grafico e user unit; riutilizzare il core senza AppHandle. Installer abilita l'unita nel contesto dell'utente normale; altri utenti possono abilitarla quando configurano l'automatismo, senza privilegi.
- L'agente riconcilia file/revisione, inventory fisica e stato osservato. Applica solo configurazioni enabled e autoApply, con lock e CAS lato bridge, identita/capabilities attuali e retry limitato. Cambiamento owner invalida la cache.
- Solo la sessione grafica locale attiva puo acquisire il mouse; niente multi-seat in questa versione. Logout/cambio utente rilascia solo injector propri, lasciando enabled per il login successivo. Non interferire con altri utenti o remapper esterni.
- Applica GUI e agente condividono locking e revisione. Disattivazione salva prima enabled=false/autoApply=false sotto lock, poi stop selettivo verificato: non deve essere riapplicata immediatamente dal worker.
- X/tray e Esci non fermano agente o engine privato. request_exit mantiene il cleanup audio esistente e termina soltanto la GUI. Non disattivare OpenRazer o altri servizi condivisi.
- Pausa l'automatismo su stop volontario con device ancora connesso, errori persistenti o due scollegamenti rapidi di emergenza; evitare restart loop. Un nuovo Apply esplicito riattiva. Fornire `peripheral-hub-remapping-agent --disable-all` e una voce desktop Ripristina mouse che disabilitino solo le configurazioni dell'app.

### 6. Pacchetti e Installer Completi

- Aggiungere `packaging/` con rootfs risorse private, user/system unit, policy DBus, fonti/patch/attribuzioni, ricetta Arch, metadati DEB/RPM per Fedora e openSUSE, script build/install e manifest dei target. Riutilizzare i bundle Tauri, aggiungendo agente e backend privato; AppImage rimane portabile ma non viene descritta come installer dei servizi.
- Ogni bundle completo contiene un pacchetto nativo dell'app con GUI, agente e engine privato, installer entry point e manifest verificabile. Nessuna build AUR richiesta agli utenti, nessuna dipendenza mancante affidata a un comando manuale.
- Derivare versioni app da manifest esistenti; non modificare arbitrariamente 0.1.0. Estrarre dipendenze ELF/Python effettive tramite tooling nativo e controllo di import. Includere WebKitGTK 4.1, GTK/appindicator, Python >=3.10, evdev, psutil, dasbus, Cairo/PyGObject, Pydantic compatibile, packaging e GtkSource richiesti dal core.
- Risolvere automaticamente anche PipeWire/WirePlumber/libmysofa, UPower, libratbag/ratbagd e OpenRazer daemon+DKMS. Mappare package manager e nomi per target, interrogando repo firmati e metadati reali, non presumendo equivalenza dei nomi tra distro.
- HeadsetControl deve restare compatibile con l'attuale contratto JSON dell'app. Verificare CLI/API e ABI del target; se il repo non fornisce una versione conforme, includere un binario privato da release/sorgente congelata e licenziata. Preferire il CLI di sistema conforme dove gia presente; nessun downgrade, overwrite o ampliamento dei filtri vendor in questo task.
- Entry point non-root: rilevare os-release/arch/init, verificare manifest, preflight e mostrare pacchetti/fonti/servizi/cambiamenti; ottenere autorizzazione; configurare eventuali repo upstream firmati; effettuare transazione nativa e fase sistema; tornare all'utente per enable/start delle user unit. Niente shell remota via curl-pipe, bypass firme, build root o systemctl --user eseguito come root per un altro utente.
- Arch/CachyOS: pacman e pacchetti ufficiali per OpenRazer/libratbag; Input Remapper e integrato nel pacchetto privato, non una dipendenza AUR irrisolta. Ubuntu/Debian: apt e repo stabile ufficiale/upstream OpenRazer solo se necessario. Fedora: dnf e repo hardware:Razer ufficiale. openSUSE: zypper/OBS e dkms.service come richiesto dall'upstream.
- Risolvere headers del kernel effettivo e verificare DKMS prima di dichiarare pronta la parte Razer. CachyOS usa il pkgbase reale, non linux-headers per supposizione; Fedora richiede kernel-devel corretto, non kernel-debug-devel. Non installare un altro kernel o disabilitare Secure Boot. Gruppi/udev/MOK e logout/reboot necessari devono risultare esplicitamente nella schermata finale.
- Installazione completa proposta per tutti i backend, ma nessuna sostituzione silenziosa PulseAudio/session manager o probe ratbag senza consenso: mostrare tali cambiamenti prima della transazione. Se l'utente rifiuta, indicare componente non configurato, non successo completo.
- Configurazione idempotente e riparazione: registrare cosa e stato creato, non sovrascrivere configurazioni esistenti e distinguere installed/running/pending-reboot/failed. Disinstallazione ferma solo componenti/injector privati dell'app; conserva dati utente salvo purge esplicito e non rimuove/spenge servizi esterni preesistenti.

### 7. Build, Test e Consegna

- Aggiungere pipeline/script Linux per build e packaging in ambienti puliti delle cinque famiglie. Registrare distro/versione/ABI nel manifest; produrre checksum, fonti GPL corrispondenti e artefatti firmabili. Pubblicazione release/repo/AUR, chiavi nuove, push e commit non sono autorizzati da questo piano.
- Aggiornare README esistente con installazione completa, requisiti/reboot, riparazione, recovery, matrice reale e distinzione AppImage/installer. Niente documentazione separata non necessaria.
- Non fermarsi a dropdown, stub IPC o liste di dipendenze: consegnare bridge, agente e risorse effettivamente inclusi, con verifiche riportate e gli eventuali target ancora non verificati chiaramente indicati.

## Confini dei File

Modifiche previste: `Controls.tsx`, `MousePanel.tsx`, `src/api.ts`, `src/types.ts`, `src/styles.css`, test frontend; `models.rs`, `commands.rs`, `lib.rs`, `devices.rs`, `profiles.rs` per riuso, nuovo core remapping e binario agente; Cargo per binario/dependency strettamente necessari; `tauri.conf.json` per risorse/metadati; nuovo `packaging/` e test/pipeline Linux. HeadsetControl/process.rs solo se serve selezionare il CLI compatibile privato, senza cambiare parser/supporto vendor. Non toccare audio/surround/driver Razer implementativi per aggiungere nuovi modelli.

## Validazione Obbligatoria

- Frontend: input DPI valido/vuoto/out-of-range/step/discreto, X/Y, reset, readback, cambio mouse e nessun IPC durante digitazione; azioni/bozze, accessibilita e sorgenti non rilevate; demo e Windows unsupported.
- Rust/Python: schema/percorsi/permessi/locking/hash, protocollo/owner/epoch, CAS e credenziali DBus, sorgenti reali/virtuali/duplicate, strict origin e grab totale, release dei modificatori, preset estraneo mai fermato, disabled non riapplicato, daemon restart, hotplug/porta debole, stop/emergency, sessione inattiva.
- Eseguire npm test/build, cargo fmt/clippy/test desktop e no-default-features e test del bridge/core upstream rilevanti. Segnalare separatamente fallimenti baseline senza correggerli fuori scope.
- Installer: lint e fixture distro/kernel/repo, checksum, pacchetti in ambienti puliti, dipendenze davvero risolte, installazione/upgrade/repair/idempotenza/fallimento parziale/uninstall. Verificare separazione root/user e servizi statici rispetto a quelli enableable.
- VM Linux/systemd: login con GUI chiusa, apply, Esci, reconnect e reboot; un solo agente e injector, recovery funzionante, stack audio/configurazioni esterne preservati, DKMS/gruppi/reboot descritti correttamente.
- Hardware reale X11 e Wayland con mouse Razer e generico: clic/scorciatoie e zero eventi duplicati; DPI hardware non rimappabili se non emettono input; motion/DPI/polling non degradati dal forwarding. Non dichiarare supportata una distro/ABI o un dispositivo senza verifica.

## Fonti e Stato Attuale

- Input Remapper stable: https://github.com/sezanzeb/input-remapper/tree/2.2.1 ; commit congelato verificato via API GitHub. pyproject richiede Python >=3.10; mapping.py supporta Pydantic v1/v2; injector.py documenta fallback, grab e stati UNKNOWN/STARTING/RUNNING/STOPPED/FAILED/NO_GRAB.
- https://github.com/sezanzeb/input-remapper/blob/main/inputremapper/daemon.py e groups.py: API del ramo attuale, identita e instabilita dei nomi di gruppi.
- https://dasbus.readthedocs.io/en/latest/api/dasbus.server.interface.html: accepts_additional_arguments/call_info per credenziali del chiamante.
- https://openrazer.github.io/#download: repo delle cinque famiglie, headers, plugdev, reboot, Secure Boot e dkms.service su openSUSE.
- Cataloghi Arch verificati: openrazer-daemon/driver e libratbag in extra; headsetcontrol 4.1.0 in extra, versione CLI non prova di compatibilita JSON. Input Remapper upstream e AUR, evitato per l'installer con componente privato.
- https://github.com/Sapd/HeadsetControl/releases/tag/4.2.0 e README upstream: artefatti firmati, source/DEB/RPM, requisiti di compilazione e udev. Validare il contratto JSON esistente prima di scegliere il binario.
- In questa fase e stato modificato solo questo piano. Ambiente Windows senza WSL disponibile: nessuna build/installazione o prova runtime Linux eseguita. Niente decisioni utente bloccanti residue; restano le verifiche implementative esplicitamente elencate.
