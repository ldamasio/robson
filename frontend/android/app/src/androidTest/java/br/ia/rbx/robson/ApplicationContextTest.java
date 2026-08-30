package br.ia.rbx.robson;

import static org.junit.Assert.assertEquals;

import android.content.Context;
import androidx.test.ext.junit.runners.AndroidJUnit4;
import androidx.test.platform.app.InstrumentationRegistry;
import org.junit.Test;
import org.junit.runner.RunWith;

@RunWith(AndroidJUnit4.class)
public class ApplicationContextTest {

    @Test
    public void applicationContextUsesRobsonPackage() {
        Context appContext = InstrumentationRegistry.getInstrumentation().getTargetContext();

        assertEquals("br.ia.rbx.robson", appContext.getPackageName());
    }
}
