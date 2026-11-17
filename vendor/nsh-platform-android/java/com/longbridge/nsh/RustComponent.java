package com.longbridge.nsh;

import android.app.Activity;
import android.content.Context;
import android.content.ContextWrapper;
import android.content.pm.ActivityInfo;
import android.hardware.input.InputManager;
import android.view.InputDevice;
import android.view.KeyEvent;
import android.view.MotionEvent;
import android.view.SurfaceHolder;
import android.view.SurfaceView;
import android.view.View;
import android.view.ViewParent;
import java.util.ArrayList;

/**
 * 本仓库加的：Rust 写的原生组件（{@code native.rs}）。视图是一个 {@link SurfaceView}，它的画面、
 * 属性、command、硬件按键和手柄的轴都交给 Rust。UI 线程。
 *
 * <p>属性 {@code screenOrientation}（{@code "landscape"}、{@code "portrait"}）在 Java 里处理：锁住 Activity 的方向，
 * 组件没了时放开。
 */
final class RustComponent implements NshComponent, SurfaceHolder.Callback, InputManager.InputDeviceListener {
    /** {@code MotionEvent} 的轴，和 {@code native.rs} 的 {@code AXES} 同一个顺序。 */
    private static final int[] AXES = {
        MotionEvent.AXIS_X, MotionEvent.AXIS_Y, MotionEvent.AXIS_Z, MotionEvent.AXIS_RZ,
        MotionEvent.AXIS_HAT_X, MotionEvent.AXIS_HAT_Y, MotionEvent.AXIS_LTRIGGER, MotionEvent.AXIS_RTRIGGER,
        MotionEvent.AXIS_BRAKE, MotionEvent.AXIS_GAS, MotionEvent.AXIS_RX, MotionEvent.AXIS_RY,
    };
    /** 活着的组件：按键和手柄的轴按 {@code NshView} 分给它们。 */
    private static final ArrayList<RustComponent> live = new ArrayList<>();

    private final NshView host;
    private final SurfaceView view;
    private final InputManager input;
    private long ptr;
    /** 锁过 Activity 的方向。 */
    private boolean oriented;

    RustComponent(NshView host, int id, String name) {
        this.host = host;
        Context context = host.getContext();
        view = new SurfaceView(context);
        float density = context.getResources().getDisplayMetrics().density;
        ptr = Native.rustComponentCreate(host.handle, id, name, context.getApplicationContext(),
                context.getCodeCacheDir().getPath(), density);
        view.getHolder().addCallback(this);
        input = (InputManager) context.getSystemService(Context.INPUT_SERVICE);
        if (input != null) input.registerInputDeviceListener(this, null);
        live.add(this);
    }

    @Override
    public View view() {
        return view;
    }

    @Override
    public void setProp(String name, Object value) {
        if ("screenOrientation".equals(name)) {
            orientation(value);
            return;
        }
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
    public void onInputDeviceAdded(int deviceId) {}

    @Override
    public void onInputDeviceChanged(int deviceId) {}

    @Override
    public void onInputDeviceRemoved(int deviceId) {
        if (ptr != 0) Native.rustComponentInputLost(ptr, deviceId, false);
    }

    @Override
    public void destroy() {
        live.remove(this);
        view.getHolder().removeCallback(this);
        if (input != null) input.unregisterInputDeviceListener(this);
        if (oriented) orientation(null);
        long p = ptr;
        ptr = 0;
        if (p != 0) Native.rustComponentDestroy(p);
    }

    /** 锁住（{@code "landscape"}、{@code "portrait"}，跟着传感器转 180°）或放开 Activity 的方向。 */
    private void orientation(Object value) {
        Activity activity = activity(host.getContext());
        if (activity == null) return;
        int o = "landscape".equals(value) ? ActivityInfo.SCREEN_ORIENTATION_SENSOR_LANDSCAPE
                : "portrait".equals(value) ? ActivityInfo.SCREEN_ORIENTATION_SENSOR_PORTRAIT
                : ActivityInfo.SCREEN_ORIENTATION_UNSPECIFIED;
        oriented = o != ActivityInfo.SCREEN_ORIENTATION_UNSPECIFIED;
        activity.setRequestedOrientation(o);
    }

    private static Activity activity(Context c) {
        while (c instanceof ContextWrapper) {
            if (c instanceof Activity) return (Activity) c;
            c = ((ContextWrapper) c).getBaseContext();
        }
        return null;
    }

    /** 组件在看得见的地方（不在的时候键交回 NSH）。 */
    private boolean takes(NshView host) {
        return this.host == host && ptr != 0 && view.isShown();
    }

    /** 焦点在输入框里（{@link InputBox}，或者是个文本编辑器）：在打字。 */
    private static boolean editing(NshView host) {
        View focus = host.findFocus();
        if (focus == null) return false;
        if (focus.onCheckIsTextEditor()) return true;
        for (View v = focus; v != null && v != host; ) {
            if (v instanceof InputBox) return true;
            ViewParent parent = v.getParent();
            v = parent instanceof View ? (View) parent : null;
        }
        return false;
    }

    /** 给 {@code host} 里的组件看一个按键；有一个吞了就是 true。 */
    static boolean key(NshView host, KeyEvent e) {
        int action = e.getAction();
        if (action != KeyEvent.ACTION_DOWN && action != KeyEvent.ACTION_UP) return false;
        boolean editing = editing(host);
        boolean taken = false;
        for (RustComponent c : new ArrayList<>(live)) {
            if (!c.takes(host)) continue;
            taken |= Native.rustComponentKey(c.ptr, e.getKeyCode(), e.getSource(), action == KeyEvent.ACTION_DOWN,
                    e.getRepeatCount(), e.getMetaState(), e.getDeviceId(), editing);
        }
        return taken;
    }

    /** 手柄（摇杆）的移动交给 {@code host} 里的组件；设备没有的轴是 NaN。 */
    static boolean motion(NshView host, MotionEvent e) {
        if ((e.getSource() & InputDevice.SOURCE_JOYSTICK) != InputDevice.SOURCE_JOYSTICK
                || e.getAction() != MotionEvent.ACTION_MOVE) {
            return false;
        }
        InputDevice device = e.getDevice();
        float[] axes = new float[AXES.length];
        for (int i = 0; i < AXES.length; i++) {
            boolean has = device == null || device.getMotionRange(AXES[i], e.getSource()) != null;
            axes[i] = has ? e.getAxisValue(AXES[i]) : Float.NaN;
        }
        boolean taken = false;
        for (RustComponent c : new ArrayList<>(live)) {
            if (!c.takes(host)) continue;
            taken |= Native.rustComponentMotion(c.ptr, e.getDeviceId(), axes);
        }
        return taken;
    }

    /** {@code host} 的窗口失焦：组件按着的都松开。 */
    static void focusLost(NshView host) {
        for (RustComponent c : new ArrayList<>(live)) {
            if (c.host == host && c.ptr != 0) Native.rustComponentInputLost(c.ptr, 0, true);
        }
    }
}
