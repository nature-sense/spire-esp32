/**
 * @file lv_conf.h — **this library's** LVGL configuration.
 *
 * LVGL decides nothing for itself: it needs an `lv_conf.h` on the include path, and this is ours.
 * `LV_CONF_INCLUDE_SIMPLE` — set by `components/lvgl/CMakeLists.txt` — is what lets LVGL find it by
 * bare name instead of by a path relative to `lvgl.h`, which is why the header may sit here in this
 * component rather than beside the vendored source.
 *
 * It is **short on purpose.** Every symbol this file does not name, LVGL fills from its own
 * `lv_conf_internal.h`. Only a setting *this library* decides belongs here; a default nobody has
 * looked at does not, and copying all fourteen hundred lines of the template would turn a decision
 * into a coincidence.
 *
 * The template this is drawn from is the vendored `upstream/lv_conf_template.h`, at the pinned tag.
 */

#ifndef LV_CONF_H
#define LV_CONF_H

/*============================================================================
 * THE LIBRARY'S DECISIONS
 *============================================================================*/

/* **RGB565**, because that is the panel's format: `st7789::flush()` pushes big-endian RGB565, and the
 * display driver hands LVGL's draw buffer straight through. Changing this without changing the
 * panel's pixel format is the classic "everything is green" screen.
 *
 * Named `LV_COLOR_FORMAT_DEFAULT`, and deliberately **not** `LV_COLOR_DEPTH`: v9.6 deprecated that
 * name and warns on every build that uses it, and LVGL derives `LV_COLOR_DEPTH` (16, here) from this
 * one for the drivers that still ask. Naming the old symbol would mean a warning per build to be
 * removed later; naming this one means the panel's format stated once. */
#define LV_COLOR_FORMAT_DEFAULT LV_COLOR_FORMAT_RGB565

/* **FreeRTOS**, because two tasks touch LVGL — the one that pumps `lv_timer_handler()`, and the actor
 * that updates a label when a reading arrives. This is what makes `lv_lock()` / `lv_unlock()` real,
 * and without it those two tasks would be drawing at once. */
#define LV_USE_OS LV_OS_FREERTOS

/* The pool LVGL allocates its widgets from. Four labels, a screen and a theme is far less than this;
 * the headroom is for a screen that grows. It is a number to *decide*, not to inherit. */
#define LV_MEM_SIZE (64U * 1024U)

/* **Off, deliberately.** A bring-up symptom is worth a serial line; a shipping screen is not worth the
 * code. Turn it on here, at a level, when something needs watching — and turn it off again. */
#define LV_USE_LOG 0

#endif /* LV_CONF_H */
