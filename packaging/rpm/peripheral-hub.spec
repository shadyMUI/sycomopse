Name:           peripheral-hub
Version:        %{ph_version}
Release:        1
Summary:        Peripheral Hub GUI, user agent and private remapping engine
License:        MIT AND GPL-3.0-or-later AND BSD-3-Clause
URL:            https://github.com/shadyMUI/sycomopse
Source0:        rootfs.tar.gz
BuildArch:      x86_64
AutoReqProv:    yes
%include %{ph_requires}

%description
The Tauri app, a non-graphical user agent and a separate private GPL Input
Remapper engine. This package does not replace stock Input Remapper services,
audio configuration, OpenRazer or ratbag configuration. The online installer
resolves and explicitly activates the distribution backends.

%prep
%setup -q -c -T
tar -xzf %{SOURCE0}

%install
mkdir -p %{buildroot}
cp -a usr %{buildroot}/usr

%post
if [ -d /run/systemd/system ]; then
    systemctl daemon-reload
fi

%preun
if [ "$1" -eq 0 ] && [ -d /run/systemd/system ]; then
    systemctl stop peripheral-hub-input-remapper.service || :
fi

%postun
if [ -d /run/systemd/system ]; then
    systemctl daemon-reload
fi

%files -f %{ph_filelist}
