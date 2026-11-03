package dev.linktolinux.companion
import org.junit.Assert.*
import org.junit.Test
import java.net.InetSocketAddress
import java.nio.channels.ServerSocketChannel
import java.io.ByteArrayInputStream
import java.io.ByteArrayOutputStream
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit
class FileWireTest {
 @Test fun metadataBounds() {
  assertEquals("a.txt", FileTransferPolicy.name("a.txt"))
  assertNull(FileTransferPolicy.size(org.json.JSONObject().put("size", org.json.JSONObject.NULL)))
  assertEquals(8589934592L, FileTransferPolicy.size(org.json.JSONObject().put("size", 8589934592L)))
  assertThrows(IllegalArgumentException::class.java) { FileTransferPolicy.size(org.json.JSONObject().put("size", "3")) }
  assertThrows(IllegalArgumentException::class.java) { FileTransferPolicy.id("ABCDEF00-0000-0000-0000-000000000000") }
  assertThrows(IllegalArgumentException::class.java) { FileTransferPolicy.name("é".repeat(121)) }
  for (name in listOf("../a", "a/b", "a\\b", " ", ".", "a\u0000")) assertThrows(IllegalArgumentException::class.java) { FileTransferPolicy.name(name) }
  assertThrows(IllegalArgumentException::class.java) { FileTransferPolicy.size(org.json.JSONObject().put("size", 1.5)) }
  assertThrows(IllegalArgumentException::class.java) { FileTransferPolicy.size(org.json.JSONObject().put("size", 8589934593L)) }
 }
 @Test fun roundtripIncludingEmpty() {
  for (bytes in listOf(ByteArray(0), ByteArray(150003) { (it % 251).toByte() })) ServerSocketChannel.open().use { server ->
   server.bind(InetSocketAddress("127.0.0.1", 0)); val pool = Executors.newSingleThreadExecutor()
   try {
    val result = pool.submit<ByteArray> { TimedFileSocket.accept(server.accept(), 2000).use { wire ->
     val out = ByteArrayOutputStream(); val receipt = FileWire.receive(wire, out, if (bytes.isEmpty()) 0L else null) {}
     wire.json(org.json.JSONObject().put("type", "file_complete").put("size", receipt.size).put("sha256", receipt.sha256)); out.toByteArray()
    } }
    TimedFileSocket.connect("127.0.0.1", (server.localAddress as InetSocketAddress).port, 2000).use { wire -> FileWire.send(wire, ByteArrayInputStream(bytes), null) {} }
    assertArrayEquals(bytes, result.get(4, TimeUnit.SECONDS))
   } finally { pool.shutdownNow() }
  }
 }
 @Test fun readDeadlineAndCancellation() { ServerSocketChannel.open().use { server ->
  server.bind(InetSocketAddress("127.0.0.1", 0))
  TimedFileSocket.connect("127.0.0.1", (server.localAddress as InetSocketAddress).port, 80).use { wire -> server.accept().use {
   assertThrows(java.net.SocketTimeoutException::class.java) { wire.int() }; wire.close(); assertThrows(java.io.IOException::class.java) { wire.int() }
  } }
 } }
 @Test fun rejectsCorruptionAndTruncation() {
  for (mode in listOf("corrupt", "truncate", "oversize")) ServerSocketChannel.open().use { server ->
   server.bind(InetSocketAddress("127.0.0.1", 0)); val pool = Executors.newSingleThreadExecutor()
   try {
    val result = pool.submit<Boolean> { TimedFileSocket.accept(server.accept(), 500).use { wire ->
     try { FileWire.receive(wire, ByteArrayOutputStream(), 3L) {}; false } catch (error: Exception) { true }
    } }
    TimedFileSocket.connect("127.0.0.1", (server.localAddress as InetSocketAddress).port, 500).use { wire ->
     when (mode) {
      "oversize" -> wire.int(65537)
      "truncate" -> { wire.int(3); wire.bytes(byteArrayOf(1)) }
      else -> { wire.int(3); wire.bytes(byteArrayOf(1, 2, 3)); wire.int(0); wire.json(org.json.JSONObject().put("type", "file_end").put("size", 3).put("sha256", "0".repeat(64))) }
     }
    }
    assertTrue(result.get(2, TimeUnit.SECONDS))
   } finally { pool.shutdownNow() }
  }
 }
 @Test fun writeDeadlineBoundsBlockedPeer() { ServerSocketChannel.open().use { server ->
  server.bind(InetSocketAddress("127.0.0.1", 0))
  TimedFileSocket.connect("127.0.0.1", (server.localAddress as InetSocketAddress).port, 100).use { client ->
   val accepted = server.accept(); accepted.socket().sendBufferSize = 1024
   TimedFileSocket.accept(accepted, 100).use { writer ->
    assertThrows(java.net.SocketTimeoutException::class.java) { repeat(10000) { writer.bytes(ByteArray(65536)) } }
   }
   client.close()
  }
 } }
 @Test fun closingSocketInterruptsPendingRead() { ServerSocketChannel.open().use { server ->
  server.bind(InetSocketAddress("127.0.0.1", 0)); val pool = Executors.newSingleThreadExecutor()
  try {
   TimedFileSocket.connect("127.0.0.1", (server.localAddress as InetSocketAddress).port, 5000).use { wire ->
    server.accept().use {
     val future = pool.submit<Boolean> { try { wire.int(); false } catch (error: Exception) { true } }
     wire.close(); assertTrue(future.get(1, TimeUnit.SECONDS))
    }
   }
  } finally { pool.shutdownNow() }
 } }

 @Test fun senderRequiresVerifiedFinalAcknowledgement() { ServerSocketChannel.open().use { server ->
  server.bind(InetSocketAddress("127.0.0.1", 0)); val pool = Executors.newSingleThreadExecutor()
  try {
   val peer = pool.submit { TimedFileSocket.accept(server.accept(), 1000).use { wire ->
    assertEquals(0, wire.int()); assertEquals("file_end", wire.json().getString("type"))
    wire.json(org.json.JSONObject().put("type", "file_complete").put("size", 0).put("sha256", "0".repeat(64)))
   } }
   TimedFileSocket.connect("127.0.0.1", (server.localAddress as InetSocketAddress).port, 1000).use { wire ->
    assertThrows(IllegalArgumentException::class.java) { FileWire.send(wire, ByteArrayInputStream(ByteArray(0)), 0L) {} }
   }
   peer.get(2, TimeUnit.SECONDS)
  } finally { pool.shutdownNow() }
 } }

}
