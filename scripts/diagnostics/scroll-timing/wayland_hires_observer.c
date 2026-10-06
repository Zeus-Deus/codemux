#include <gtk/gtk.h>
#include <gdk/gdkwayland.h>
#include <wayland-client.h>
#include <stdio.h>
#include <stdint.h>
#include <string.h>

/* Diagnostic only: a second wl_pointer on GTK's existing connection, no new
 * connection/input device reads, no event replacement, no custom dispatch. */
struct Observer {
    struct wl_seat *seat;
    struct wl_pointer *pointer;
    uint32_t name, source, time;
    struct wl_surface *surface;
    double x, y, axis_x, axis_y;
    int32_t v120_x, v120_y;
    gboolean axis_seen, v120_seen;
};
static GtkWidget *window;
static struct wl_surface *own_surface;
static GPtrArray *observers;
static gint64 start;
static void stamp(void) { printf("%.3f ", (g_get_monotonic_time()-start)/1000.0); }
static void enter(void *d, struct wl_pointer *p, uint32_t serial, struct wl_surface *s, wl_fixed_t x, wl_fixed_t y) {
    struct Observer *o=d; o->surface=s; o->x=wl_fixed_to_double(x); o->y=wl_fixed_to_double(y);
    stamp(); printf("ENTER seat=%u own=%d xy=%.2f,%.2f\n", o->name, s==own_surface,o->x,o->y);
}
static void leave(void *d, struct wl_pointer *p, uint32_t serial, struct wl_surface *s) { ((struct Observer*)d)->surface=NULL; }
static void motion(void *d, struct wl_pointer *p, uint32_t time, wl_fixed_t x, wl_fixed_t y) { struct Observer *o=d; o->time=time;o->x=wl_fixed_to_double(x);o->y=wl_fixed_to_double(y); }
static void button(void *d, struct wl_pointer *p, uint32_t serial, uint32_t time, uint32_t button, uint32_t state) {}
static void axis(void *d, struct wl_pointer *p, uint32_t time, uint32_t axis, wl_fixed_t value) {
    struct Observer *o=d; o->time=time;o->axis_seen=TRUE;
    if (axis==WL_POINTER_AXIS_VERTICAL_SCROLL) o->axis_y+=wl_fixed_to_double(value); else o->axis_x+=wl_fixed_to_double(value);
}
static void frame(void *d, struct wl_pointer *p) {
    struct Observer *o=d;
    if (o->axis_seen || o->v120_seen) {
        stamp(); printf("WAYLAND seat=%u own=%d time=%u source=%u axis=%.5f,%.5f value120=%d,%d seen=%d\n",o->name,o->surface==own_surface,o->time,o->source,o->axis_x,o->axis_y,o->v120_x,o->v120_y,o->v120_seen);
    }
    o->axis_x=o->axis_y=0;o->v120_x=o->v120_y=0;o->axis_seen=o->v120_seen=FALSE;
}
static void source(void *d, struct wl_pointer *p, uint32_t source) { ((struct Observer*)d)->source=source; }
static void stop(void *d, struct wl_pointer *p, uint32_t time, uint32_t axis) {}
static void discrete(void *d, struct wl_pointer *p, uint32_t axis, int32_t value) {}
static void value120(void *d, struct wl_pointer *p, uint32_t axis, int32_t value) { struct Observer *o=d; o->v120_seen=TRUE;if(axis==WL_POINTER_AXIS_VERTICAL_SCROLL)o->v120_y+=value;else o->v120_x+=value; }
static const struct wl_pointer_listener pointer_listener={.enter=enter,.leave=leave,.motion=motion,.button=button,.axis=axis,.frame=frame,.axis_source=source,.axis_stop=stop,.axis_discrete=discrete,.axis_value120=value120};
static void caps(void *d, struct wl_seat *s, uint32_t capabilities) {
    struct Observer *o=d;
    if ((capabilities & WL_SEAT_CAPABILITY_POINTER) && !o->pointer) { o->pointer=wl_seat_get_pointer(s);wl_pointer_add_listener(o->pointer,&pointer_listener,o); stamp();printf("POINTER seat=%u version=%u\n",o->name,wl_proxy_get_version((struct wl_proxy*)o->pointer)); }
    else if (!(capabilities & WL_SEAT_CAPABILITY_POINTER) && o->pointer) { wl_pointer_release(o->pointer);o->pointer=NULL;o->surface=NULL; }
}
static void seatname(void *d, struct wl_seat *s, const char *name) {}
static const struct wl_seat_listener seat_listener={.capabilities=caps,.name=seatname};
static void global(void *d, struct wl_registry *r, uint32_t name,const char *interface,uint32_t version) {
    if(strcmp(interface,"wl_seat") || version<8 || wl_seat_interface.version<8 || wl_pointer_interface.version<8)return;
    struct Observer *o=g_new0(struct Observer,1);o->name=name;o->source=UINT32_MAX;
    o->seat=wl_registry_bind(r,name,&wl_seat_interface,8);wl_seat_add_listener(o->seat,&seat_listener,o);g_ptr_array_add(observers,o);
    stamp();printf("SEAT name=%u advertised=%u bound=8\n",name,version);
}
static void removed(void *d, struct wl_registry *r, uint32_t name) {
    for(guint i=0;i<observers->len;i++){struct Observer *o=g_ptr_array_index(observers,i);if(o->name!=name)continue;if(o->pointer)wl_pointer_release(o->pointer);o->pointer=NULL;wl_seat_release(o->seat);o->seat=NULL;o->surface=NULL;}
}
static const struct wl_registry_listener registry_listener={.global=global,.global_remove=removed};
static gboolean scroll(GtkWidget *w,GdkEventScroll *e,void *unused) {
    double dx=0,dy=0;gdk_event_get_scroll_deltas((GdkEvent*)e,&dx,&dy);
    stamp();printf("GDK time=%u direction=%u delta=%.5f,%.5f emulated=%d source=%d\n",e->time,e->direction,dx,dy,gdk_event_get_pointer_emulated((GdkEvent*)e),gdk_device_get_source(gdk_event_get_source_device((GdkEvent*)e)));return TRUE;
}
static gboolean finish(void *d){gtk_main_quit();return G_SOURCE_REMOVE;}
int main(int argc,char **argv) {
    setvbuf(stdout,NULL,_IOLBF,0);start=g_get_monotonic_time();gtk_init(&argc,&argv);
    GdkDisplay *display=gdk_display_get_default();if(!GDK_IS_WAYLAND_DISPLAY(display)){fprintf(stderr,"Wayland required\n");return 2;}
    observers=g_ptr_array_new_with_free_func(g_free);
    struct wl_display *wd=gdk_wayland_display_get_wl_display(display);
    struct wl_registry *registry=wl_display_get_registry(wd);wl_registry_add_listener(registry,&registry_listener,NULL);
    wl_display_roundtrip(wd);wl_display_roundtrip(wd);
    window=gtk_window_new(GTK_WINDOW_TOPLEVEL);gtk_window_set_focus_on_map(GTK_WINDOW(window),FALSE);gtk_window_set_accept_focus(GTK_WINDOW(window),FALSE);gtk_window_set_title(GTK_WINDOW(window),"CodeMux high-resolution wheel diagnostic");gtk_window_set_default_size(GTK_WINDOW(window),720,360);
    gtk_container_add(GTK_CONTAINER(window),gtk_label_new("Temporary read-only diagnostic\n\nScroll over this window: it logs the same mouse through GTK3\nand a version-8 Wayland pointer on GTK's connection."));
    gtk_widget_add_events(window,GDK_SCROLL_MASK|GDK_SMOOTH_SCROLL_MASK);g_signal_connect(window,"scroll-event",G_CALLBACK(scroll),NULL);g_signal_connect(window,"destroy",G_CALLBACK(gtk_main_quit),NULL);
    gtk_widget_realize(window);own_surface=gdk_wayland_window_get_wl_surface(gtk_widget_get_window(window));gtk_widget_show_all(window);
    g_timeout_add_seconds(argc>1?atoi(argv[1]):60,finish,NULL);gtk_main();
    for(guint i=0;i<observers->len;i++){struct Observer *o=g_ptr_array_index(observers,i);if(o->pointer)wl_pointer_release(o->pointer);if(o->seat)wl_seat_release(o->seat);}wl_registry_destroy(registry);wl_display_flush(wd);g_ptr_array_free(observers,TRUE);return 0;
}
