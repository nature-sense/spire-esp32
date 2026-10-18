# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (c) 2026 NatureSense
#
# ft6336 — a **protocol driver**: one device (a capacitive touch controller), one bus (I²C).
#
# REQUIRES names the IDF driver this device speaks through, and nothing else. A protocol driver knows
# its bus and its datasheet; it knows nothing about a board, an actor, or a product. The *panel's*
# facts — its grid — live here; the *board's* — the interrupt line, the reset line, the bus
# it hangs off — do not.

# What this component **is**, stated rather than guessed. The same rule the project follows with
# `SPIRE_PROJECT_STRUCTURE`: nothing else in the tree says which kind of component this is.
set(SPIRE_COMPONENT_KIND driver)

idf_component_register(
    SRCS "src/ft6336.cpp"
    INCLUDE_DIRS "include"
    REQUIRES esp_driver_i2c
)
