package com.topjohnwu.magisk.test

import androidx.annotation.Keep
import androidx.test.ext.junit.runners.AndroidJUnit4
import com.topjohnwu.magisk.core.model.module.LocalModule
import com.topjohnwu.magisk.core.utils.RootUtils
import com.topjohnwu.magisk.test.Environment.Companion.MOUNT_TEST
import com.topjohnwu.magisk.test.Environment.Companion.REMOVE_TEST
import com.topjohnwu.magisk.test.Environment.Companion.SEPOLICY_RULE
import com.topjohnwu.magisk.test.Environment.Companion.UPGRADE_TEST
import com.topjohnwu.superuser.Shell
import kotlinx.coroutines.runBlocking
import org.junit.After
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Assume.assumeTrue
import org.junit.BeforeClass
import org.junit.Test
import org.junit.runner.RunWith


@Keep
@RunWith(AndroidJUnit4::class)
class AdditionalTest : BaseTest {

    companion object {

        private lateinit var modules: List<LocalModule>

        @BeforeClass
        @JvmStatic
        fun before() {
            BaseTest.prerequisite()
            runBlocking {
                modules = LocalModule.installed()
            }
        }
    }

    @After
    fun teardown() {
        device.pressHome()
    }

    @Test
    fun testModuleCount() {
        var expected = 4
        if (Environment.mount()) expected++
        if (Environment.preinit()) expected++
        if (Environment.lsposed()) expected++
        if (Environment.shamiko()) expected++
        assertEquals("Module count incorrect", expected, modules.size)
    }

    @Test
    fun testModuleMount() {
        assumeTrue(Environment.mount())

        assertNotNull("$MOUNT_TEST is not installed", modules.find { it.id == MOUNT_TEST })
        assertTrue(
            "/system/fonts/newfile should exist",
            RootUtils.fs.getFile("/system/fonts/newfile").exists()
        )
        assertFalse(
            "/system/bin/screenrecord should not exist",
            RootUtils.fs.getFile("/system/bin/screenrecord").exists()
        )
        val egg = RootUtils.fs.getFile("/system/app/EasterEgg").list() ?: arrayOf()
        assertArrayEquals(
            "/system/app/EasterEgg should be replaced",
            egg,
            arrayOf("newfile")
        )
    }

    @Test
    fun testSepolicyRule() {
        assumeTrue(Environment.preinit())

        assertNotNull("$SEPOLICY_RULE is not installed", modules.find { it.id == SEPOLICY_RULE })
        assertTrue(
            "Module sepolicy.rule is not applied",
            Shell.cmd("magiskpolicy --print-rules | grep -q magisk_test").exec().isSuccess
        )
    }

    @Test
    fun testRemoveModule() {
        assertNull("$REMOVE_TEST should be removed", modules.find { it.id == REMOVE_TEST })
        assertTrue(
            "Uninstaller of $REMOVE_TEST should be run",
            RootUtils.fs.getFile(Environment.REMOVE_TEST_MARKER).exists()
        )
    }

    @Test
    fun testModuleUpgrade() {
        val module = modules.find { it.id == UPGRADE_TEST }
        assertNotNull("$UPGRADE_TEST is not installed", module)
        module!!
        assertFalse("$UPGRADE_TEST should be disabled", module.enable)
        assertTrue(
            "$UPGRADE_TEST should be updated",
            module.base.getChildFile("post-fs-data.sh").exists()
        )
        assertFalse(
            "$UPGRADE_TEST should be updated",
            module.base.getChildFile("service.sh").exists()
        )
    }
}
