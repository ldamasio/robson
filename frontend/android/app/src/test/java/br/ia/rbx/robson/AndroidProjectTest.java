package br.ia.rbx.robson;

import static org.junit.Assert.assertEquals;

import org.junit.Test;

public class AndroidProjectTest {

    @Test
    public void applicationIdRemainsStable() {
        assertEquals("br.ia.rbx.robson", MainActivity.class.getPackageName());
    }
}
