# 四语言查询题集与标准答案 v1

由固定源码和独立标注生成，48 道主问题／32 个 Constrain 配对／8 组边界。未人工复核；未标注内容保持未知。

输入 SHA-256：`dcac8aef61e1c08716182483587f2bae6e068ab7717fa10f64d01387d483aa21`。精确原文、字节及范围以 `data/g4-compiled-v1.json` 为准。

## go:Q-01：behavior

How does validation of a braced UUID differ from permissive parsing?

Validate 在 38 字节形式下明确检查首尾花括号；Parse 对这一长度只跳过首字节，随后检查中间 UUID 的格式。因此解析成功不能代替严格括号校验。

必需证据：

- `uuid.go`，UTF-8 字节 `[5640, 5773)`：

```text
case 36 + 2:
		if s[0] != '{' || s[len(s)-1] != '}' {
			return fmt.Errorf("invalid bracketed UUID format")
		}
		s = s[1 : len(s)-1]
```

- `uuid.go`，UTF-8 字节 `[2390, 2414)`：

```text
case 36 + 2:
		s = s[1:]
```

## go:Q-02：behavior

What happens when a byte slice has the wrong length for constructing a UUID?

FromBytes 把输入交给 UnmarshalBinary 并返回其错误；UnmarshalBinary 要求恰好 16 字节，长度错误时返回带实际长度的错误，成功时复制字节。接收者调用的图目标仍按当前不推断边界处理。

必需证据：

- `uuid.go`，UTF-8 字节 `[4776, 4876)`：

```text
func FromBytes(b []byte) (uuid UUID, err error) {
	err = uuid.UnmarshalBinary(b)
	return uuid, err
}
```

- `marshal.go`，UTF-8 字节 `[730, 906)`：

```text
func (uuid *UUID) UnmarshalBinary(data []byte) error {
	if len(data) != 16 {
		return fmt.Errorf("invalid UUID (got %d bytes)", len(data))
	}
	copy(uuid[:], data)
	return nil
}
```

## go:Q-03：behavior

How are the version and variant bits set when a UUID is generated from a hash?

NewHash 先取哈希前 16 字节，再把第 6 字节高四位设为给定版本低四位，把第 8 字节高位设为 RFC 4122 变体。

必需证据：

- `hash.go`，UTF-8 字节 `[1315, 1440)`：

```text
copy(uuid[:], s)
	uuid[6] = (uuid[6] & 0x0f) | uint8((version&0xf)<<4)
	uuid[8] = (uuid[8] & 0x3f) | 0x80 // RFC 4122 variant
```

## go:Q-04：calls

Which parsing entry converts a returned parse error into a panic?

MustParse 调用 Parse，err 非空时构造 panic 信息；Parse 在输入长度不符合已支持形式时返回 invalidLengthError。

必需证据：

- `uuid.go`，UTF-8 字节 `[4484, 4623)`：

```text
func MustParse(s string) UUID {
	uuid, err := Parse(s)
	if err != nil {
		panic(`uuid: Parse(` + s + `): ` + err.Error())
	}
	return uuid
}
```

- `uuid.go`，UTF-8 字节 `[2633, 2683)`：

```text
default:
		return uuid, invalidLengthError{len(s)}
```

Constrain：`MustParse`，来源 `uuid.go` 字节 `4484`；范围内必需区间 2 个。

## go:Q-05：calls

Which shared formatter supplies the text-marshaling implementation?

MarshalText 在 36 字节缓冲区上调用 encodeHex；encodeHex 负责十六进制分组与连字符的位置。

必需证据：

- `marshal.go`，UTF-8 字节 `[235, 345)`：

```text
func (uuid UUID) MarshalText() ([]byte, error) {
	var js [36]byte
	encodeHex(js[:], uuid)
	return js[:], nil
}
```

- `uuid.go`，UTF-8 字节 `[6834, 7099)`：

```text
func encodeHex(dst []byte, uuid UUID) {
	hex.Encode(dst, uuid[:4])
	dst[8] = '-'
	hex.Encode(dst[9:13], uuid[4:6])
	dst[13] = '-'
	hex.Encode(dst[14:18], uuid[6:8])
	dst[18] = '-'
	hex.Encode(dst[19:23], uuid[8:10])
	dst[23] = '-'
	hex.Encode(dst[24:], uuid[10:])
}
```

Constrain：`MarshalText`，来源 `marshal.go` 字节 `235`；范围内必需区间 2 个。

## go:Q-06：calls

Which parser is called by the text unmarshaler, and when is the destination updated?

UnmarshalText 调用 ParseBytes；有错误即返回，成功后才通过 *uuid = id 更新接收者。ParseBytes 的长度分派和格式检查在 uuid.go 中。

必需证据：

- `marshal.go`，UTF-8 字节 `[401, 541)`：

```text
func (uuid *UUID) UnmarshalText(data []byte) error {
	id, err := ParseBytes(data)
	if err != nil {
		return err
	}
	*uuid = id
	return nil
}
```

- `uuid.go`，UTF-8 字节 `[3825, 3875)`：

```text
default:
		return uuid, invalidLengthError{len(b)}
```

Constrain：`UnmarshalText`，来源 `marshal.go` 字节 `401`；范围内必需区间 2 个。

## go:Q-07：dependencies

How do the MD5 and SHA-1 UUID factories delegate algorithm-independent work?

NewMD5 和 NewSHA1 分别建立对应哈希器，把版本 3／5 和相同的 namespace/data 交给 NewHash；公共函数负责输入组合、哈希截取及位设置。

必需证据：

- `hash.go`，UTF-8 字节 `[1618, 1706)`：

```text
func NewMD5(space UUID, data []byte) UUID {
	return NewHash(md5.New(), space, data, 3)
}
```

- `hash.go`，UTF-8 字节 `[1872, 1962)`：

```text
func NewSHA1(space UUID, data []byte) UUID {
	return NewHash(sha1.New(), space, data, 5)
}
```

- `hash.go`，UTF-8 字节 `[1198, 1298)`：

```text
h.Reset()
	h.Write(space[:]) //nolint:errcheck
	h.Write(data)     //nolint:errcheck
	s := h.Sum(nil)
```

Constrain：`NewMD5`，来源 `hash.go` 字节 `1618`；范围内必需区间 2 个。

## go:Q-08：dependencies

How do the string form and text marshaler keep UUID punctuation consistent?

String 与 MarshalText 都使用 encodeHex；二者都准备 36 字节目标，连字符布局统一由该 helper 设置。

必需证据：

- `uuid.go`，UTF-8 字节 `[6474, 6576)`：

```text
func (uuid UUID) String() string {
	var buf [36]byte
	encodeHex(buf[:], uuid)
	return string(buf[:])
}
```

- `marshal.go`，UTF-8 字节 `[235, 345)`：

```text
func (uuid UUID) MarshalText() ([]byte, error) {
	var js [36]byte
	encodeHex(js[:], uuid)
	return js[:], nil
}
```

- `uuid.go`，UTF-8 字节 `[6902, 7064)`：

```text
dst[8] = '-'
	hex.Encode(dst[9:13], uuid[4:6])
	dst[13] = '-'
	hex.Encode(dst[14:18], uuid[6:8])
	dst[18] = '-'
	hex.Encode(dst[19:23], uuid[8:10])
	dst[23] = '-'
```

## go:Q-09：chain

Trace rejection of a non-hex UUID from the panic-on-error entry to byte decoding.

MustParse → Parse → xtob。xtob 通过查表与 255 哨兵返回是否为有效十六进制；Parse 在 !ok 时返回格式错误，外层 MustParse 在 err 非空时 panic。

必需证据：

- `uuid.go`，UTF-8 字节 `[4484, 4623)`：

```text
func MustParse(s string) UUID {
	uuid, err := Parse(s)
	if err != nil {
		panic(`uuid: Parse(` + s + `): ` + err.Error())
	}
	return uuid
}
```

- `uuid.go`，UTF-8 字节 `[3010, 3116)`：

```text
v, ok := xtob(s[x], s[x+1])
		if !ok {
			return uuid, errors.New("invalid UUID format")
		}
		uuid[i] = v
```

- `util.go`，UTF-8 字节 `[1795, 1919)`：

```text
func xtob(x1, x2 byte) (byte, bool) {
	b1 := xvalues[x1]
	b2 := xvalues[x2]
	return (b1 << 4) | b2, b1 != 255 && b2 != 255
}
```

Constrain：`MustParse`，来源 `uuid.go` 字节 `4484`；范围内必需区间 3 个。

## go:Q-10：chain

Trace a failed textual UUID decode before it can overwrite the destination value.

UnmarshalText → ParseBytes → xtob。无效十六进制导致 ParseBytes 返回错误，UnmarshalText 在赋值前返回；目标只在成功路径被覆盖。

必需证据：

- `marshal.go`，UTF-8 字节 `[401, 541)`：

```text
func (uuid *UUID) UnmarshalText(data []byte) error {
	id, err := ParseBytes(data)
	if err != nil {
		return err
	}
	*uuid = id
	return nil
}
```

- `uuid.go`，UTF-8 字节 `[4202, 4308)`：

```text
v, ok := xtob(b[x], b[x+1])
		if !ok {
			return uuid, errors.New("invalid UUID format")
		}
		uuid[i] = v
```

- `util.go`，UTF-8 字节 `[1795, 1919)`：

```text
func xtob(x1, x2 byte) (byte, bool) {
	b1 := xvalues[x1]
	b2 := xvalues[x2]
	return (b1 << 4) | b2, b1 != 255 && b2 != 255
}
```

Constrain：`UnmarshalText`，来源 `marshal.go` 字节 `401`；范围内必需区间 3 个。

## go:Q-11：responsibility

How does the fixed-size UUID type relate to its binary marshal and unmarshal contracts?

UUID 的底层表示是 [16]byte。MarshalBinary 返回其字节切片；UnmarshalBinary 要求 16 字节后复制。这不要求推断任何隐式接口实现。

必需证据：

- `uuid.go`，UTF-8 字节 `[353, 371)`：

```text
type UUID [16]byte
```

- `marshal.go`，UTF-8 字节 `[597, 670)`：

```text
func (uuid UUID) MarshalBinary() ([]byte, error) {
	return uuid[:], nil
}
```

- `marshal.go`，UTF-8 字节 `[730, 906)`：

```text
func (uuid *UUID) UnmarshalBinary(data []byte) error {
	if len(data) != 16 {
		return fmt.Errorf("invalid UUID (got %d bytes)", len(data))
	}
	copy(uuid[:], data)
	return nil
}
```

Constrain：`UnmarshalBinary`，来源 `marshal.go` 字节 `730`；范围内必需区间 1 个。

## go:Q-12：responsibility

How is an invalid-length parse error represented and recognized?

invalidLengthError 保存 len，Error 把它格式化为文本；IsInvalidLengthError 用该具体类型断言识别错误。该职责不同于按字符串猜测错误类别。

必需证据：

- `uuid.go`，UTF-8 字节 `[1106, 1147)`：

```text
type invalidLengthError struct{ len int }
```

- `uuid.go`，UTF-8 字节 `[1149, 1253)`：

```text
func (err invalidLengthError) Error() string {
	return fmt.Sprintf("invalid UUID length: %d", err.len)
}
```

- `uuid.go`，UTF-8 字节 `[1335, 1426)`：

```text
func IsInvalidLengthError(err error) bool {
	_, ok := err.(invalidLengthError)
	return ok
}
```

Constrain：`IsInvalidLengthError`，来源 `uuid.go` 字节 `1335`；范围内必需区间 1 个。

## java:Q-01：behavior

What prevents operations from using a value after it has been destroyed?

checkNotDestroyed 检查 destroyed 并抛 RuntimeException；isNull 等方法先调用这一检查再调用 Native。这是 Java 层检查，不证明 native 层实现。

必需证据：

- `tools/java_api/src/main/java/com/kuzudb/Value.java`，UTF-8 字节 `[1333, 1464)`：

```text
public void checkNotDestroyed() {
        if (destroyed)
            throw new RuntimeException("Value has been destroyed.");
    }
```

- `tools/java_api/src/main/java/com/kuzudb/Value.java`，UTF-8 字节 `[2323, 2428)`：

```text
public boolean isNull() {
        checkNotDestroyed();
        return Native.kuzuValueIsNull(this);
    }
```

## java:Q-02：behavior

When does closing a value release native resources and mark it destroyed?

close 委托 destroy；destroy 先检查未销毁，仅在 !isOwnedByCPP 时调用 Native.kuzuValueDestroy 并设置 destroyed=true。

必需证据：

- `tools/java_api/src/main/java/com/kuzudb/Value.java`，UTF-8 字节 `[1712, 1772)`：

```text
@Override
    public void close() {
        destroy();
    }
```

- `tools/java_api/src/main/java/com/kuzudb/Value.java`，UTF-8 字节 `[1964, 2135)`：

```text
private void destroy() {
        checkNotDestroyed();
        if (!isOwnedByCPP) {
            Native.kuzuValueDestroy(this);
            destroyed = true;
        }
    }
```

## java:Q-03：behavior

How does cloning an already destroyed value differ from cloning a live one?

Value.clone 在 destroyed 时返回 null，否则调用 Native.kuzuValueClone；Native.java 只提供本地方法声明，题集不推断 JNI 实现。

必需证据：

- `tools/java_api/src/main/java/com/kuzudb/Value.java`，UTF-8 字节 `[3114, 3251)`：

```text
public Value clone() {
        if (destroyed)
            return null;
        else
            return Native.kuzuValueClone(this);
    }
```

- `tools/java_api/src/main/java/com/kuzudb/Native.java`，UTF-8 字节 `[7437, 7495)`：

```text
protected static native Value kuzuValueClone(Value value);
```

## java:Q-04：calls

Which test calls the empty-value factory, and what ownership and type checks follow?

ValueCreateNull 调用 Value.createNull，验证非 CPP 所有、类型为 ANY，并在转换验证后 close。工厂委托本地方法声明。实例调用目标不由该题强行推断。

必需证据：

- `tools/java_api/src/test/java/com/kuzudb/ValueTest.java`，UTF-8 字节 `[966, 1218)`：

```text
@Test
    void ValueCreateNull() {
        Value value = Value.createNull();
        assertFalse(value.isOwnedByCPP());
        assertEquals(value.getDataType().getID(), DataTypeID.ANY);
        checkValueConversion(value);
        value.close();
    }
```

- `tools/java_api/src/main/java/com/kuzudb/Value.java`，UTF-8 字节 `[534, 619)`：

```text
public static Value createNull() {
        return Native.kuzuValueCreateNull();
    }
```

Constrain：`ValueCreateNull`，来源 `tools/java_api/src/test/java/com/kuzudb/ValueTest.java` 字节 `966`；范围内必需区间 2 个。

## java:Q-05：calls

Which test exercises a typed null factory and when does it release the temporary type?

ValueCreateNullWithDatatype 创建 INT64 的 DataType，调用 createNullWithDataType，检查所有权后关闭临时 type，再检查值类型并转换、关闭值。

必需证据：

- `tools/java_api/src/test/java/com/kuzudb/ValueTest.java`，UTF-8 字节 `[1224, 1593)`：

```text
@Test
    void ValueCreateNullWithDatatype() {
        DataType type = new DataType(DataTypeID.INT64, null, 0);
        Value value = Value.createNullWithDataType(type);
        assertFalse(value.isOwnedByCPP());
        type.close();
        assertEquals(value.getDataType().getID(), DataTypeID.INT64);
        checkValueConversion(value);
        value.close();
    }
```

- `tools/java_api/src/main/java/com/kuzudb/Value.java`，UTF-8 字节 `[759, 895)`：

```text
public static Value createNullWithDataType(DataType data_type) {
        return Native.kuzuValueCreateNullWithDataType(data_type);
    }
```

Constrain：`ValueCreateNullWithDatatype`，来源 `tools/java_api/src/test/java/com/kuzudb/ValueTest.java` 字节 `1224`；范围内必需区间 2 个。

## java:Q-06：calls

Which native declaration is called when a value is cloned?

Value.clone 的存活分支调用 Native.kuzuValueClone(this)，其 Java 声明返回 Value；声明不等于 native 实现体。

必需证据：

- `tools/java_api/src/main/java/com/kuzudb/Value.java`，UTF-8 字节 `[3114, 3251)`：

```text
public Value clone() {
        if (destroyed)
            return null;
        else
            return Native.kuzuValueClone(this);
    }
```

- `tools/java_api/src/main/java/com/kuzudb/Native.java`，UTF-8 字节 `[7437, 7495)`：

```text
protected static native Value kuzuValueClone(Value value);
```

Constrain：`clone`，来源 `tools/java_api/src/main/java/com/kuzudb/Value.java` 字节 `3114`；范围内必需区间 1 个。

## java:Q-07：dependencies

How does typed-null construction cross the Java wrapper and native declaration boundary?

Value.createNullWithDataType 把 DataType 参数原样交给 Native 的同名静态 native 声明；这里能证明 Java 调用和声明，不证明 JNI 内部行为。

必需证据：

- `tools/java_api/src/main/java/com/kuzudb/Value.java`，UTF-8 字节 `[759, 895)`：

```text
public static Value createNullWithDataType(DataType data_type) {
        return Native.kuzuValueCreateNullWithDataType(data_type);
    }
```

- `tools/java_api/src/main/java/com/kuzudb/Native.java`，UTF-8 字节 `[7057, 7138)`：

```text
protected static native Value kuzuValueCreateNullWithDataType(DataType dataType);
```

Constrain：`createNullWithDataType`，来源 `tools/java_api/src/main/java/com/kuzudb/Value.java` 字节 `759`；范围内必需区间 1 个。

## java:Q-08：dependencies

How do value cloning and data-type cloning depend on different native entry declarations?

两种 clone 都在 destroyed 时返回 null，但分别委托 kuzuValueClone(Value) 和 kuzuDataTypeClone(DataType)，返回相应包装类型。

必需证据：

- `tools/java_api/src/main/java/com/kuzudb/Value.java`，UTF-8 字节 `[3114, 3251)`：

```text
public Value clone() {
        if (destroyed)
            return null;
        else
            return Native.kuzuValueClone(this);
    }
```

- `tools/java_api/src/main/java/com/kuzudb/DataType.java`，UTF-8 字节 `[1645, 1788)`：

```text
public DataType clone() {
        if (destroyed)
            return null;
        else
            return Native.kuzuDataTypeClone(this);
    }
```

- `tools/java_api/src/main/java/com/kuzudb/Native.java`，UTF-8 字节 `[7437, 7495)`：

```text
protected static native Value kuzuValueClone(Value value);
```

- `tools/java_api/src/main/java/com/kuzudb/Native.java`，UTF-8 字节 `[6490, 6560)`：

```text
protected static native DataType kuzuDataTypeClone(DataType dataType);
```

## java:Q-09：chain

Trace closing a data-type wrapper through its guard and state transition.

DataType.close → destroy → checkNotDestroyed；检查通过后调用 Native.kuzuDataTypeDestroy，再把 destroyed 置 true。不对实例派发补充超出支持范围的图边。

必需证据：

- `tools/java_api/src/main/java/com/kuzudb/DataType.java`，UTF-8 字节 `[1200, 1260)`：

```text
@Override
    public void close() {
        destroy();
    }
```

- `tools/java_api/src/main/java/com/kuzudb/DataType.java`，UTF-8 字节 `[1406, 1533)`：

```text
private void destroy() {
        checkNotDestroyed();
        Native.kuzuDataTypeDestroy(this);
        destroyed = true;
    }
```

- `tools/java_api/src/main/java/com/kuzudb/DataType.java`，UTF-8 字节 `[801, 936)`：

```text
private void checkNotDestroyed() {
        if (destroyed)
            throw new RuntimeException("DataType has been destroyed.");
    }
```

Constrain：`close`，来源 `tools/java_api/src/main/java/com/kuzudb/DataType.java` 字节 `1200`；范围内必需区间 1 个。

## java:Q-10：chain

How does the cloning test check that a cloned value survives closing the original?

ValueClone 在取得 clone 后关闭原 value，随后验证 clone 的所有权、字符串类型和原内容，最后关闭 clone；Value.clone 的存活路径交给 native 声明。测试是上游源码证据，本轮不声称执行了 Java 测试。

必需证据：

- `tools/java_api/src/test/java/com/kuzudb/ValueTest.java`，UTF-8 字节 `[10775, 11038)`：

```text
Value clone = value.clone();
        value.close();

        assertFalse(clone.isOwnedByCPP());
        assertEquals(clone.getDataType().getID(), DataTypeID.STRING);
        str = clone.getValue();
        assertTrue(str.equals("abcdefg"));
        clone.close();
```

- `tools/java_api/src/main/java/com/kuzudb/Value.java`，UTF-8 字节 `[3114, 3251)`：

```text
public Value clone() {
        if (destroyed)
            return null;
        else
            return Native.kuzuValueClone(this);
    }
```

Constrain：`ValueClone`，来源 `tools/java_api/src/test/java/com/kuzudb/ValueTest.java` 字节 `10497`；范围内必需区间 1 个。

## java:Q-11：responsibility

Which resource wrappers explicitly declare automatic close support, and where do their close methods delegate?

Value 和 DataType 均明确 implements AutoCloseable，close 都委托各自 destroy。AutoCloseable 来自外部 JDK，题集只标注声明，不伪造本地实现目标。

必需证据：

- `tools/java_api/src/main/java/com/kuzudb/Value.java`，UTF-8 字节 `[72, 115)`：

```text
public class Value implements AutoCloseable
```

- `tools/java_api/src/main/java/com/kuzudb/DataType.java`，UTF-8 字节 `[92, 138)`：

```text
public class DataType implements AutoCloseable
```

- `tools/java_api/src/main/java/com/kuzudb/Value.java`，UTF-8 字节 `[1712, 1772)`：

```text
@Override
    public void close() {
        destroy();
    }
```

- `tools/java_api/src/main/java/com/kuzudb/DataType.java`，UTF-8 字节 `[1200, 1260)`：

```text
@Override
    public void close() {
        destroy();
    }
```

Constrain：`Value`，来源 `tools/java_api/src/main/java/com/kuzudb/Value.java` 字节 `72`；范围内必需区间 2 个。

## java:Q-12：responsibility

Where does the value test inherit its database and connection setup?

ValueTest 显式 extends TestBase；基类声明共享 db／conn 和 getDBandConn，设置方法通过 TestHelper 建立它们。TestHelper 的实现不在此子集。

必需证据：

- `tools/java_api/src/test/java/com/kuzudb/ValueTest.java`，UTF-8 字节 `[318, 357)`：

```text
public class ValueTest extends TestBase
```

- `tools/java_api/src/test/java/com/kuzudb/TestBase.java`，UTF-8 字节 `[279, 384)`：

```text
protected static Database db;
    protected static Connection conn;
    @TempDir
    static Path tempDir;
```

- `tools/java_api/src/test/java/com/kuzudb/TestBase.java`，UTF-8 字节 `[390, 638)`：

```text
@BeforeAll
    static void getDBandConn() throws IOException {
        String dbPath = tempDir.resolve("db.kz").toString();
        TestHelper.loadData(dbPath);
        db = TestHelper.getDatabase();
        conn = TestHelper.getConnection();
    }
```

Constrain：`ValueTest`，来源 `tools/java_api/src/test/java/com/kuzudb/ValueTest.java` 字节 `318`；范围内必需区间 3 个。

## c:Q-01：behavior

How does formatting an error distinguish a missing destination, a formatting failure, and truncation?

ArrowErrorSet 在 error=NULL 时返回成功；否则清空 message、格式化，vsnprintf 负值返回 EINVAL，需要字符数超过缓冲区返回 ERANGE，其余成功。

必需证据：

- `src/nanoarrow/common/utils.c`，UTF-8 字节 `[1096, 1591)`：

```text
ArrowErrorCode ArrowErrorSet(struct ArrowError* error, const char* fmt, ...) {
  if (error == NULL) {
    return NANOARROW_OK;
  }

  memset(error->message, 0, sizeof(error->message));

  va_list args;
  va_start(args, fmt);
  int chars_needed = vsnprintf(error->message, sizeof(error->message), fmt, args);
  va_end(args);

  if (chars_needed < 0) {
    return EINVAL;
  } else if (((size_t)chars_needed) >= sizeof(error->message)) {
    return ERANGE;
  } else {
    return NANOARROW_OK;
  }
}
```

## c:Q-02：behavior

How does schema initialization roll back when assigning its requested type fails?

ArrowSchemaInitFromType 先初始化 schema，再设置类型；非成功时调用 ArrowSchemaRelease 并返回原错误。头文件的 Release helper 通过回调执行释放并检查 release 清空，回调目标不推断。

必需证据：

- `src/nanoarrow/common/schema.c`，UTF-8 字节 `[7225, 7504)`：

```text
ArrowErrorCode ArrowSchemaInitFromType(struct ArrowSchema* schema, enum ArrowType type) {
  ArrowSchemaInit(schema);

  int result = ArrowSchemaSetType(schema, type);
  if (result != NANOARROW_OK) {
    ArrowSchemaRelease(schema);
    return result;
  }

  return NANOARROW_OK;
}
```

- `src/nanoarrow/common/inline_types.h`，UTF-8 字节 `[11868, 12045)`：

```text
static inline void ArrowSchemaRelease(struct ArrowSchema* schema) {
  NANOARROW_DCHECK(schema != NULL);
  schema->release(schema);
  NANOARROW_DCHECK(schema->release == NULL);
}
```

## c:Q-03：behavior

How does the basic array stream signal end of input without returning another array?

GetNext 检查 stream 与 release 后，arrays_i==n_arrays 时把输出 array->release 置 NULL 并返回成功，不再移动数组。

必需证据：

- `src/nanoarrow/common/array_stream.c`，UTF-8 字节 `[1560, 1643)`：

```text
if (array_stream == NULL || array_stream->release == NULL) {
    return EINVAL;
  }
```

- `src/nanoarrow/common/array_stream.c`，UTF-8 字节 `[1764, 1875)`：

```text
if (private_data->arrays_i == private_data->n_arrays) {
    array->release = NULL;
    return NANOARROW_OK;
  }
```

## c:Q-04：calls

Which allocator callback releases a non-null buffer through the local free wrapper?

ArrowBufferAllocatorMallocFree 仅对非空 ptr 调用同编译单元 ArrowFree；后者转交外部 free。不能把 libc 的实现当作本地源码。

必需证据：

- `src/nanoarrow/common/utils.c`，UTF-8 字节 `[7907, 8166)`：

```text
static void ArrowBufferAllocatorMallocFree(struct ArrowBufferAllocator* allocator,
                                           uint8_t* ptr, int64_t size) {
  NANOARROW_UNUSED(allocator);
  NANOARROW_UNUSED(size);
  if (ptr != NULL) {
    ArrowFree(ptr);
  }
}
```

- `src/nanoarrow/common/utils.c`，UTF-8 字节 `[7599, 7639)`：

```text
void ArrowFree(void* ptr) { free(ptr); }
```

Constrain：`ArrowBufferAllocatorMallocFree`，来源 `src/nanoarrow/common/utils.c` 字节 `7907`；范围内必需区间 2 个。

## c:Q-05：calls

Which functions prepare an empty schema before its requested type is assigned?

ArrowSchemaInitFromType 调用 ArrowSchemaInit 再调用 ArrowSchemaSetType；前者清空字段并登记内部 release 回调，后者选择格式并初始化需要的子结构。宏包裹的调用与直接调用分开标注。

必需证据：

- `src/nanoarrow/common/schema.c`，UTF-8 字节 `[7317, 7391)`：

```text
ArrowSchemaInit(schema);

  int result = ArrowSchemaSetType(schema, type);
```

- `src/nanoarrow/common/schema.c`，UTF-8 字节 `[5865, 6193)`：

```text
void ArrowSchemaInit(struct ArrowSchema* schema) {
  schema->format = NULL;
  schema->name = NULL;
  schema->metadata = NULL;
  schema->flags = ARROW_FLAG_NULLABLE;
  schema->n_children = 0;
  schema->children = NULL;
  schema->dictionary = NULL;
  schema->private_data = NULL;
  schema->release = &ArrowSchemaReleaseInternal;
}
```

Constrain：`ArrowSchemaInitFromType`，来源 `src/nanoarrow/common/schema.c` 字节 `7225`；范围内必需区间 2 个。

## c:Q-06：calls

How does the basic stream obtain an independent schema for a consumer?

GetSchema 检查 stream 是否有效，从 private_data 取 schema 并调用 ArrowSchemaDeepCopy；深拷贝在另一个编译单元逐步复制字段、子项和字典。此子集不证明跨编译单元链接目标。

必需证据：

- `src/nanoarrow/common/array_stream.c`，UTF-8 字节 `[991, 1409)`：

```text
static int ArrowBasicArrayStreamGetSchema(struct ArrowArrayStream* array_stream,
                                          struct ArrowSchema* schema) {
  if (array_stream == NULL || array_stream->release == NULL) {
    return EINVAL;
  }

  struct BasicArrayStreamPrivate* private_data =
      (struct BasicArrayStreamPrivate*)array_stream->private_data;
  return ArrowSchemaDeepCopy(&private_data->schema, schema);
}
```

- `src/nanoarrow/common/schema.c`，UTF-8 字节 `[17171, 17265)`：

```text
ArrowSchemaInit(schema_out);

  int result = ArrowSchemaSetFormat(schema_out, schema->format);
```

Constrain：`ArrowBasicArrayStreamGetSchema`，来源 `src/nanoarrow/common/array_stream.c` 字节 `991`；范围内必需区间 1 个。

## c:Q-07：dependencies

Where is the default allocator declared publicly and where is its implementation supplied?

nanoarrow.h 声明 ArrowBufferAllocatorDefault；utils.c 的定义返回静态默认分配器记录。声明与定义分别留证，不把原型当作可调用实现。

必需证据：

- `src/nanoarrow/nanoarrow.h`，UTF-8 字节 `[9104, 9180)`：

```text
NANOARROW_DLL struct ArrowBufferAllocator ArrowBufferAllocatorDefault(void);
```

- `src/nanoarrow/common/utils.c`，UTF-8 字节 `[8318, 8420)`：

```text
struct ArrowBufferAllocator ArrowBufferAllocatorDefault(void) {
  return ArrowBufferAllocatorMalloc;
}
```

Constrain：`ArrowBufferAllocatorDefault`，来源 `src/nanoarrow/common/utils.c` 字节 `8318`；范围内必需区间 1 个。

## c:Q-08：dependencies

How does replacing a schema format use the separately implemented memory wrappers?

ArrowSchemaSetFormat 先通过 ArrowFree 释放旧 format，再按 strlen+1 通过 ArrowMalloc 分配并复制新字符串；分配失败返回 ENOMEM。两个 wrapper 的定义在 utils.c，跨单元绑定未作承诺。

必需证据：

- `src/nanoarrow/common/schema.c`，UTF-8 字节 `[14539, 14801)`：

```text
if (schema->format != NULL) {
    ArrowFree((void*)schema->format);
  }

  if (format != NULL) {
    size_t format_size = strlen(format) + 1;
    schema->format = (const char*)ArrowMalloc(format_size);
    if (schema->format == NULL) {
      return ENOMEM;
    }
```

- `src/nanoarrow/common/utils.c`，UTF-8 字节 `[7465, 7521)`：

```text
void* ArrowMalloc(int64_t size) { return malloc(size); }
```

- `src/nanoarrow/common/utils.c`，UTF-8 字节 `[7599, 7639)`：

```text
void ArrowFree(void* ptr) { free(ptr); }
```

- `src/nanoarrow/common/schema.c`，UTF-8 字节 `[14807, 14858)`：

```text
memcpy((void*)schema->format, format, format_size);
```

## c:Q-09：chain

Trace initialization, type assignment, and failure cleanup for a new schema.

InitFromType → Init／SetType；SetType 对未知非 UNINITIALIZED 类型返回 EINVAL，外层遇错通过 inline Release helper 调用 release 回调。宏展开与回调派发不冒充已解析静态图边。

必需证据：

- `src/nanoarrow/common/schema.c`，UTF-8 字节 `[7225, 7504)`：

```text
ArrowErrorCode ArrowSchemaInitFromType(struct ArrowSchema* schema, enum ArrowType type) {
  ArrowSchemaInit(schema);

  int result = ArrowSchemaSetType(schema, type);
  if (result != NANOARROW_OK) {
    ArrowSchemaRelease(schema);
    return result;
  }

  return NANOARROW_OK;
}
```

- `src/nanoarrow/common/schema.c`，UTF-8 字节 `[6565, 6658)`：

```text
if (template_format == NULL && type != NANOARROW_TYPE_UNINITIALIZED) {
    return EINVAL;
  }
```

- `src/nanoarrow/common/inline_types.h`，UTF-8 字节 `[11868, 12045)`：

```text
static inline void ArrowSchemaRelease(struct ArrowSchema* schema) {
  NANOARROW_DCHECK(schema != NULL);
  schema->release(schema);
  NANOARROW_DCHECK(schema->release == NULL);
}
```

Constrain：`ArrowSchemaInitFromType`，来源 `src/nanoarrow/common/schema.c` 字节 `7225`；范围内必需区间 3 个。

## c:Q-10：chain

Trace ownership transfer when the basic stream returns its next array.

GetNext 在非末尾时用 arrays_i++ 选取数组并调用 ArrowArrayMove；inline helper memcpy 整个 ArrowArray，再把来源 release 置 NULL。

必需证据：

- `src/nanoarrow/common/array_stream.c`，UTF-8 字节 `[1879, 1950)`：

```text
ArrowArrayMove(&private_data->arrays[private_data->arrays_i++], array);
```

- `src/nanoarrow/common/inline_types.h`，UTF-8 字节 `[12047, 12269)`：

```text
static inline void ArrowArrayMove(struct ArrowArray* src, struct ArrowArray* dst) {
  NANOARROW_DCHECK(src != NULL);
  NANOARROW_DCHECK(dst != NULL);

  memcpy(dst, src, sizeof(struct ArrowArray));
  src->release = NULL;
}
```

Constrain：`ArrowBasicArrayStreamGetNext`，来源 `src/nanoarrow/common/array_stream.c` 字节 `1411`；范围内必需区间 1 个。

## c:Q-11：responsibility

How does a custom deallocator populate the allocator callback record?

ArrowBufferAllocator 记录 reallocate、free 和 private_data；ArrowBufferDeallocator 把 reallocate 设为受限 helper，把 free 设为调用方传入回调，并保存 private_data。回调不是继承关系。

必需证据：

- `src/nanoarrow/common/inline_types.h`，UTF-8 字节 `[23321, 23789)`：

```text
struct ArrowBufferAllocator {
  /// \brief Reallocate a buffer or return NULL if it cannot be reallocated
  uint8_t* (*reallocate)(struct ArrowBufferAllocator* allocator, uint8_t* ptr,
                         int64_t old_size, int64_t new_size);

  /// \brief Deallocate a buffer allocated by this allocator
  void (*free)(struct ArrowBufferAllocator* allocator, uint8_t* ptr, int64_t size);

  /// \brief Opaque data specific to the allocator
  void* private_data;
}
```

- `src/nanoarrow/common/utils.c`，UTF-8 字节 `[9312, 9702)`：

```text
struct ArrowBufferAllocator ArrowBufferDeallocator(
    void (*custom_free)(struct ArrowBufferAllocator* allocator, uint8_t* ptr,
                        int64_t size),
    void* private_data) {
  struct ArrowBufferAllocator allocator;
  allocator.reallocate = &ArrowBufferDeallocatorReallocate;
  allocator.free = custom_free;
  allocator.private_data = private_data;
  return allocator;
}
```

Constrain：`ArrowBufferDeallocator`，来源 `src/nanoarrow/common/utils.c` 字节 `9312`；范围内必需区间 1 个。

## c:Q-12：responsibility

Which state belongs to a basic stream and how is its input schema ownership transferred?

BasicArrayStreamPrivate 保存 schema、n_arrays、arrays、arrays_i；初始化用 ArrowSchemaMove 把 schema 复制到私有记录并清空来源 release，随后初始化数组数量、指针和索引。

必需证据：

- `src/nanoarrow/common/array_stream.c`，UTF-8 字节 `[856, 988)`：

```text
struct BasicArrayStreamPrivate {
  struct ArrowSchema schema;
  int64_t n_arrays;
  struct ArrowArray* arrays;
  int64_t arrays_i;
}
```

- `src/nanoarrow/common/array_stream.c`，UTF-8 字节 `[3194, 3340)`：

```text
ArrowSchemaMove(schema, &private_data->schema);

  private_data->n_arrays = n_arrays;
  private_data->arrays = NULL;
  private_data->arrays_i = 0;
```

- `src/nanoarrow/common/inline_types.h`，UTF-8 字节 `[11640, 11866)`：

```text
static inline void ArrowSchemaMove(struct ArrowSchema* src, struct ArrowSchema* dst) {
  NANOARROW_DCHECK(src != NULL);
  NANOARROW_DCHECK(dst != NULL);

  memcpy(dst, src, sizeof(struct ArrowSchema));
  src->release = NULL;
}
```

Constrain：`ArrowBasicArrayStreamInit`，来源 `src/nanoarrow/common/array_stream.c` 字节 `2817`；范围内必需区间 1 个。

## cpp:Q-01：behavior

Which checks reject invalid quoting or an unfinished escape in a string literal?

DecodeStringLiteral 检查长度、开引号种类和配对尾引号；普通字符若等于开引号则拒绝未转义引号，结尾反斜线触发 incomplete string escape。

必需证据：

- `src/common/string_literal.cc`，UTF-8 字节 `[936, 1070)`：

```text
if (text.size() < 2 || (text[0] != '\'' && text[0] != '"') ||
      text.back() != text[0])
    throw Error("invalid string literal");
```

- `src/common/string_literal.cc`，UTF-8 字节 `[1165, 1384)`：

```text
if (c != '\\') {
      if (c == text.front()) throw Error("unescaped string quote");
      out += c;
      continue;
    }
    if (i + 1 >= text.size() - 1) throw Error("incomplete string escape");
    c = text.at(++i);
```

## cpp:Q-02：behavior

Which Unicode code points are rejected before UTF-8 bytes are emitted?

Utf8 拒绝大于 0x10ffff 的码点及 0xd800—0xdfff 代理区间，抛 UnicodeEscapeError；其他码点继续进入编码分支。

必需证据：

- `src/common/string_literal.cc`，UTF-8 字节 `[188, 296)`：

```text
if (cp > 0x10ffff || (cp >= 0xd800 && cp <= 0xdfff))
    throw UnicodeEscapeError("invalid Unicode escape");
```

- `src/common/string_literal.cc`，UTF-8 字节 `[299, 347)`：

```text
if (cp < 0x80)
    out += static_cast<char>(cp);
```

## cpp:Q-03：behavior

How does the string decoder choose the number of hexadecimal digits for Unicode escapes?

x／X 默认 2 位，u／U 默认 4 位；仅 U 且有足够 8 位十六进制时改为 8 位。长度不足或非十六进制抛 UnicodeEscapeError。

必需证据：

- `src/common/string_literal.cc`，UTF-8 字节 `[1899, 2554)`：

```text
size_t count = (c == 'x' || c == 'X') ? 2 : 4;
        // Kuzu accepts both 4- and 8-hex escapes. Prefer 8 only for U.
        if (c == 'U' && i + 8 < text.size() - 1 &&
            std::all_of(text.begin() + i + 1, text.begin() + i + 9,
                        [](unsigned char h) { return std::isxdigit(h); }))
          count = 8;
        if (i + count >= text.size() - 1)
          throw UnicodeEscapeError("incomplete Unicode escape");
        if (!std::all_of(text.begin() + i + 1, text.begin() + i + 1 + count,
                         [](unsigned char h) { return std::isxdigit(h); }))
          throw UnicodeEscapeError("invalid Unicode escape");
```

## cpp:Q-04：calls

Which local helper turns decoded hexadecimal code points into output bytes?

DecodeStringLiteral 在十六进制长度与字符检查后用 std::stoul 得到码点并调用同命名空间 Utf8；外部 std::stoul 的实现不在图中。

必需证据：

- `src/common/string_literal.cc`，UTF-8 字节 `[2563, 2691)`：

```text
Utf8(out, static_cast<uint32_t>(
                      std::stoul(text.substr(i + 1, count), nullptr, 16)));
        i += count;
```

- `src/common/string_literal.cc`，UTF-8 字节 `[188, 296)`：

```text
if (cp > 0x10ffff || (cp >= 0xd800 && cp <= 0xdfff))
    throw UnicodeEscapeError("invalid Unicode escape");
```

Constrain：`DecodeStringLiteral`，来源 `src/common/string_literal.cc` 字节 `875`；范围内必需区间 2 个。

## cpp:Q-05：calls

Which diagnostic-name functions are called to validate a constructed diagnostic?

DiagnosticError 构造体依次调用 ErrorCodeName、ErrorCategoryName、ErrorStageName；其中 ErrorCodeName 对未知错误码抛 logic_error。类外限定构造定义是否被图支持单列，不据文本名称补边。

必需证据：

- `src/common/error.cc`，UTF-8 字节 `[2228, 2322)`：

```text
(void)ErrorCodeName(code);
  (void)ErrorCategoryName(category);
  (void)ErrorStageName(stage);
```

- `src/common/error.cc`，UTF-8 字节 `[1174, 1224)`：

```text
throw std::logic_error("invalid diagnostic code");
```

Constrain：`DiagnosticError::DiagnosticError`，来源 `src/common/error.cc` 字节 `1884`；范围内必需区间 2 个。

## cpp:Q-06：calls

Which exception type is thrown for a malformed Unicode escape, and how is that type declared?

DecodeStringLiteral 和 Utf8 的 Unicode 错误使用 UnicodeEscapeError；头文件将其声明为 Error 的公开子类并继承构造函数，Error 再公开继承 runtime_error。构造调用不自动视为普通函数调用边。

必需证据：

- `src/common/string_literal.cc`，UTF-8 字节 `[2285, 2339)`：

```text
throw UnicodeEscapeError("incomplete Unicode escape");
```

- `include/graphdb/common/string_literal.h`，UTF-8 字节 `[244, 318)`：

```text
class UnicodeEscapeError : public Error {
 public:
  using Error::Error;
}
```

- `include/graphdb/common/error.h`，UTF-8 字节 `[218, 313)`：

```text
class Error : public std::runtime_error {
 public:
  using std::runtime_error::runtime_error;
}
```

Constrain：`DecodeStringLiteral`，来源 `src/common/string_literal.cc` 字节 `875`；范围内必需区间 1 个。

## cpp:Q-07：dependencies

How does the public string-literal header connect its decoder declaration to the error hierarchy?

string_literal.h 声明 DecodeStringLiteral，同时声明继承 Error 的 UnicodeEscapeError，并通过 include error.h 引入其基础错误声明；实现位于 string_literal.cc。

必需证据：

- `include/graphdb/common/string_literal.h`，UTF-8 字节 `[320, 377)`：

```text
std::string DecodeStringLiteral(const std::string &text);
```

- `include/graphdb/common/string_literal.h`，UTF-8 字节 `[244, 318)`：

```text
class UnicodeEscapeError : public Error {
 public:
  using Error::Error;
}
```

- `include/graphdb/common/error.h`，UTF-8 字节 `[218, 313)`：

```text
class Error : public std::runtime_error {
 public:
  using std::runtime_error::runtime_error;
}
```

- `src/common/string_literal.cc`，UTF-8 字节 `[1032, 1070)`：

```text
throw Error("invalid string literal");
```

- `include/graphdb/common/string_literal.h`，UTF-8 字节 `[31, 64)`：

```text
#include "graphdb/common/error.h"
```

Constrain：`UnicodeEscapeError`，来源 `include/graphdb/common/string_literal.h` 字节 `244`；范围内必需区间 2 个。

## cpp:Q-08：dependencies

Where do diagnostic construction and its public declaration agree on the structured fields?

error.h 中 DiagnosticError 声明 code、category、stage、optional location；error.cc 的构造初始化列表逐项保存参数。基类 Error 保存 message，不能从消息文本反推这些结构字段。

必需证据：

- `include/graphdb/common/error.h`，UTF-8 字节 `[2061, 2188)`：

```text
const ErrorCode code;
  const ErrorCategory category;
  const ErrorStage stage;
  const std::optional<SourceLocation> location;
```

- `src/common/error.cc`，UTF-8 字节 `[2106, 2225)`：

```text
: Error(std::move(message)),
      code(code),
      category(category),
      stage(stage),
      location(location) {
```

## cpp:Q-09：chain

Trace a hexadecimal string escape through validation, number conversion, and UTF-8 encoding.

解码器先检查 count 与十六进制字符，std::stoul(...,16) 转换后传给 Utf8；Utf8 再拒绝非法码点并编码。这是源码处理链，外部库实现不作为本地路径。

必需证据：

- `src/common/string_literal.cc`，UTF-8 字节 `[2241, 2691)`：

```text
if (i + count >= text.size() - 1)
          throw UnicodeEscapeError("incomplete Unicode escape");
        if (!std::all_of(text.begin() + i + 1, text.begin() + i + 1 + count,
                         [](unsigned char h) { return std::isxdigit(h); }))
          throw UnicodeEscapeError("invalid Unicode escape");
        Utf8(out, static_cast<uint32_t>(
                      std::stoul(text.substr(i + 1, count), nullptr, 16)));
        i += count;
```

- `src/common/string_literal.cc`，UTF-8 字节 `[188, 296)`：

```text
if (cp > 0x10ffff || (cp >= 0xd800 && cp <= 0xdfff))
    throw UnicodeEscapeError("invalid Unicode escape");
```

Constrain：`DecodeStringLiteral`，来源 `src/common/string_literal.cc` 字节 `875`；范围内必需区间 2 个。

## cpp:Q-10：chain

How does a structured diagnostic validate its enum values and source position?

DiagnosticError 在保存字段后调用三个枚举转换函数；若 location 存在且 line 或 column 为 0，抛一基位置错误。SourceLocation 的两个字段在头文件中定义。

必需证据：

- `src/common/error.cc`，UTF-8 字节 `[2228, 2444)`：

```text
(void)ErrorCodeName(code);
  (void)ErrorCategoryName(category);
  (void)ErrorStageName(stage);
  if (location && (!location->line || !location->column))
    throw std::logic_error("diagnostic location is one-based");
```

- `include/graphdb/common/error.h`，UTF-8 字节 `[1623, 1680)`：

```text
struct SourceLocation {
  size_t line;
  size_t column;
}
```

Constrain：`DiagnosticError::DiagnosticError`，来源 `src/common/error.cc` 字节 `1884`；范围内必需区间 1 个。

## cpp:Q-11：responsibility

How does the Unicode-specific exception fit into the shared error type hierarchy?

UnicodeEscapeError public extends Error，Error public extends std::runtime_error；UnicodeEscapeError 使用 using Error::Error 继承构造函数。前一条是子集内显式关系，runtime_error 的定义属于外部标准库。

必需证据：

- `include/graphdb/common/string_literal.h`，UTF-8 字节 `[244, 318)`：

```text
class UnicodeEscapeError : public Error {
 public:
  using Error::Error;
}
```

- `include/graphdb/common/error.h`，UTF-8 字节 `[218, 313)`：

```text
class Error : public std::runtime_error {
 public:
  using std::runtime_error::runtime_error;
}
```

Constrain：`UnicodeEscapeError`，来源 `include/graphdb/common/string_literal.h` 字节 `244`；范围内必需区间 2 个。

## cpp:Q-12：responsibility

What immutable metadata does the diagnostic exception carry beyond its message?

DiagnosticError 的 code、category、stage 和 optional SourceLocation 字段均为 const；SourceLocation 保存 line／column，构造体检查存在的位置是一基。

必需证据：

- `include/graphdb/common/error.h`，UTF-8 字节 `[2061, 2188)`：

```text
const ErrorCode code;
  const ErrorCategory category;
  const ErrorStage stage;
  const std::optional<SourceLocation> location;
```

- `include/graphdb/common/error.h`，UTF-8 字节 `[1623, 1680)`：

```text
struct SourceLocation {
  size_t line;
  size_t column;
}
```

- `src/common/error.cc`，UTF-8 字节 `[2325, 2444)`：

```text
if (location && (!location->line || !location->column))
    throw std::logic_error("diagnostic location is one-based");
```

Constrain：`DiagnosticError`，来源 `include/graphdb/common/error.h` 字节 `1826`；范围内必需区间 1 个。

## 边界用例（收益评分之外）

- **go:B-01**：未知接收者 uuid.UnmarshalBinary 不靠类型猜测目标。；预期 `unresolved`。
- **go:B-02**：node_net.go 的 !js 构建条件缺少构建上下文；独立工作区只包含该原样来源和 go.mod。；预期 `not_ready`。
- **java:B-01**：value.close 实例调用不推断接收者。；预期 `unresolved`。
- **java:B-02**：native 方法可保留 Java 声明位置，但不得生成不存在的 JNI 实现体。；预期 `declaration_only_or_unresolved`。
- **c:B-01**：ArrowFree 中的外部 free 不能误连其他同名本地函数。；预期 `unresolved`。
- **c:B-02**：allocator->free 是真实函数指针调用，不作为静态直接目标。；预期 `unresolved`。
- **cpp:B-01**：std::stoul 的外部实现不在子集，不猜测目标。；预期 `unresolved`。
- **cpp:B-02**：保留 compiler/literals.cc 默认参数在固定 grammar 下的原 v1 失败；不将其他子集通过视为修复。；预期 `not_ready`。
