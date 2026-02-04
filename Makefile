PKG_CONFIG_LIBPIPEWIRE = $$(pkg-config --cflags --libs libpipewire-0.3)

all: pipewire pipewire-rust

pipewire: src/pipewire/pipewire.h
	clang \
		$(PKG_CONFIG_LIBPIPEWIRE) \
		-Wall \
		-DPIPEWIRE_IMPLEMENTATION \
		-c \
		-x c \
		-o src/pipewire/pipewire.o \
		src/pipewire/pipewire.h
	ar rcs src/pipewire/libpipewire.a src/pipewire/pipewire.o

pipewire-rust: src/pipewire/pipewire.h
	bindgen \
		--allowlist-item "pipewire_start" \
		src/pipewire/pipewire.h >src/pipewire/bindings.rs \
		-- $(PKG_CONFIG_LIBPIPEWIRE)

.PHONY: gen_compile_commands
gen_compile_commands:
	bear -- make pipewire
