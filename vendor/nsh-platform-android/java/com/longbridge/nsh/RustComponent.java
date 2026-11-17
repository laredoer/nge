package com.longbridge.nsh;

import android.view.InputDevice;
import android.view.KeyEvent;
import android.view.MotionEvent;
import android.view.SurfaceHolder;
import android.view.SurfaceView;
import android.view.View;
import java.util.ArrayList;

/**
 * 本仓库加的：Rust 写的原生组件（{@code native.rs}）。视图是一个 {@link SurfaceView}，它的画面、
 * 属性、command、硬件按键和手柄的轴都交给 Rust。UI 线程。
 */
final class RustComponent implements NshComponent, SurfaceHolder.Callback {
    /** {@code MotionEvent} 的轴，和 {@code native.rs} 的 {@code AXES} 同一个顺序。 */
    private static final int[] AXES = {
        MotionEvent.AXIS_X, MotionEvent.AXIS_Y, MotionEvent.AXIS_Z, MotionEvent.AXIS_RZ,
        MotionEvent.AXIS_HAT_X, MotionEvent.AXIS_HAT_Y, MotionEvent.AXIS_LTRIGGER, MotionEvent.AXIS_RTRIGGER,
        MotionEvent.AXIS_BRAKE, MotionEvent.AXIS_GAS,
    };
    /** 活着的组件：按键和手柄的轴按 {@code NshView} 分给它们。 */
    private static final ArrayList<RustComponent> live = new ArrayList<>();

    private final NshView host;
    private final SurfaceView view;
    private long ptr;

    RustComponent(NshView host, int id, String name) {
        this.host = host;
        android.content.Context context = host.getContext();
        view = new SurfaceView(context);
        float density = context.getResources().getDisplayMetrics().density;
        ptr = Native.rustComponentCreate(host.handle, id, name, context.getApplicationContext(),
                context.getCodeCacheDir().getPath(), density);
        view.getHolder().addCallback(this);
        live.add(this);
    }

    @Override
    public View view() {
        return view;
    }

    @Override
    public void setProp(String name, Object value) {
        if (ptr != 0) Native.rustComponentProp(ptr, name, value == null ? null : Json.stringify(value));
    }

    @Override
    public void command(String name, Object args) {
        if (ptr != 0) Native.rustComponentCommand(ptr, name, args == null ? null : Json.stringify(args));
    }

    @Override
    public void surfaceCreated(SurfaceHolder holder) {}

    @Override
    public void surfaceChanged(SurfaceHolder holder, int format, int width, int height) {
        if (ptr != 0) Native.rustComponentSurface(ptr, holder.getSurface(), width, height);
    }

    @Override
    public void surfaceDestroyed(SurfaceHolder holder) {
        // 同步：Rust 返回时已经不再用这个画面
        if (ptr != 0) Native.rustComponentSurface(ptr, null, 0, 0);
    }

    @Override
    public void destroy() {
        live.remove(this);
        view.getHolder().removeCallback(this);
        long p = ptr;
        ptr = 0;
        if (p != 0) Native.rustComponentDestroy(p);
    }

    /** 给 {@code host} 里的组件看一个按键；有一个吞了就是 true。 */
    static boolean key(NshView host, KeyEvent e) {
        int action = e.getAction();
        if (action != KeyEvent.ACTION_DOWN && action != KeyEvent.ACTION_UP) return false;
        View focus = host.findFocus();
        boolean editing = focus != null && focus != host;
        boolean taken = false;
        for (RustComponent c : new ArrayList<>(live)) {
            if (c.host != host || c.ptr == 0) continue;
            taken |= Native.rustComponentKey(c.ptr, e.getKeyCode(), e.getSource(), action == KeyEvent.ACTION_DOWN,
                    e.getRepeatCount(), e.getMetaState(), editing);
        }
        return taken;
    }

    /** 手柄（摇杆）的移动交给 {@code host} 里的组件。 */
    static boolean motion(NshView host, MotionEvent e) {
        if ((e.getSource() & InputDevice.SOURCE_JOYSTICK) != InputDevice.SOURCE_JOYSTICK
                || e.getAction() != MotionEvent.ACTION_MOVE) {
            return false;
        }
        float[] axes = new float[AXES.length];
        for (int i = 0; i < AXES.length; i++) axes[i] = e.getAxisValue(AXES[i]);
        boolean taken = false;
        for (RustComponent c : new ArrayList<>(live)) {
            if (c.host != host || c.ptr == 0) continue;
            taken |= Native.rustComponentMotion(c.ptr, e.getDeviceId(), axes);
        }
        return taken;
    }
}
