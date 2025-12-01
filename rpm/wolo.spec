Name:           wolo
Version:        0.0.1
Release:        1%{?dist}
Summary:        Simple network monitoring in Rust.

License:        MIT OR Apache-2.0
URL:            https://github.com/udoprog/wolo
Source0:        https://github.com/udoprog/wolo/archive/refs/tags/%{version}.tar.gz

BuildRequires:  cargo
BuildRequires:  rust

%description
A simple networking utility in Rust.

%prep
%autosetup

%build
cargo build --release

%install
install -Dm755 target/release/wolo %{buildroot}%{_bindir}/wolo

%files
%license LICENSE-MIT
%license LICENSE-APACHE
%{_bindir}/wolo

%changelog
* Mon Dec 01 2025 John-John Tedro <udoprog@tedro.se> - 0.0.1-1
- Release 0.0.1

* Mon Dec 01 2025 John-John Tedro <udoprog@tedro.se> - 0.0.0-1
- Initial release 0.0.0
