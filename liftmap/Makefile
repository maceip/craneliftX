# Build the sample binary used by the performance lift/drop analyzer.
# On Apple Silicon we cross-compile to x86_64 because the whole
# native->Pulley pipeline lifts amd64 machine code.

CC ?= cc
CFLAGS = -O1 -fno-inline
UNAME := $(shell uname)
ifeq ($(UNAME),Darwin)
  CFLAGS += -arch x86_64
endif

.PHONY: all clean
all: sample_network sample_network.o

sample_network: sample_network.c
	$(CC) $(CFLAGS) sample_network.c -o sample_network

sample_network.o: sample_network.c
	$(CC) $(CFLAGS) -c sample_network.c -o sample_network.o

clean:
	rm -f sample_network sample_network.o lift_map.json
