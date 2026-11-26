%global debug_package %{nil}

Name:           peripheral-hub
Version:        %{ph_version}
Release:        1
Summary:        Sycomopse GUI, user agent and private remapping engine
License:        MIT AND GPL-3.0-or-later AND BSD-3-Clause
URL:            https://github.com/shadyMUI/sycomopse
Source0:        rootfs.tar.gz
BuildArch:      x86_64
AutoReqProv:    yes
Recommends:     openrazer-meta
Recommends:     pipewire-pulseaudio
%include %{ph_requires}

%description
The Tauri app, a non-graphical user agent and a separate private GPL Input
Remapper engine. This package does not replace stock Input Remapper services,
audio configuration, OpenRazer or ratbag configuration. Native package hooks
activate its private backend; the graphical session starts the user agent.

%prep
%setup -q -c -T
tar -xzf %{SOURCE0}

%install
mkdir -p %{buildroot}
cp -a usr %{buildroot}/usr
if [ -d etc ]; then
    cp -a etc %{buildroot}/etc
fi

%post
if [ "$1" -eq 1 ]; then
    python3 -I -B /usr/lib/peripheral-hub/installer/native.py configure --format rpm
else
    python3 -I -B /usr/lib/peripheral-hub/installer/native.py configure --format rpm --upgrade
fi

%preun
if [ "$1" -eq 0 ]; then
    python3 -I -B /usr/lib/peripheral-hub/installer/native.py remove --format rpm
fi

%postun
if [ -d /run/systemd/system ]; then
    systemctl daemon-reload
fi

%files -f %{ph_filelist}
