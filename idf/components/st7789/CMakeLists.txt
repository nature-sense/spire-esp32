# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (c) 2026 NatureSense
#
# st7789 — a **protocol driver**: one device (a colour LCD controller), one bus (SPI).
#
# REQUIRES names the IDF driver this device speaks through, and nothing else. A protocol driver knows
# its bus and its datasheet; it knows nothing about a board, an actor, or a product. The *panel's*
# facts — its geometry, its colour order, its init sequence — live in this component; the *board's* —
# the reset line, the data/command line, the backlight, the clock — do not.

# What this component **is**, stated rather than guessed. The same rule the project follows with
# `SPIRE_PROJECT_STRUCTURE`: nothing else in the tree says which kind of component this is.
set(SPIRE_COMPONENT_KIND driver)

idf_component_register(
    SRCS "src/st7789.cpp"
    INCLUDE_DIRS "include"
    REQUIRES esp_driver_spi
)
