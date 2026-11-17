package main

import (
	"encoding/hex"
	"math/rand"
	"strconv"

	"github.com/prebid/go-gdpr/bitutils"
)

// bitutilsCase records bitutils.ParseByte4/ParseByte8/ParseUInt12/ParseUInt16 for every start bit
// in [0, 8*len(data)+16].
type bitutilsCase struct {
	DataHex string   `json:"data_hex"`
	Byte4   []string `json:"byte4"`
	Byte8   []string `json:"byte8"`
	UInt12  []string `json:"uint12"`
	UInt16  []string `json:"uint16"`
}

func buildBitutilsCases() []bitutilsCase {
	r := rand.New(rand.NewSource(7))
	buffers := [][]byte{
		{0x04, 0xa2, 0x03, 0xb1, 0x00, 0x2b},
		{0x44, 0x76},
		{0x10}, {0x92}, {0x99}, {0x01, 0xe0},
		{0x60, 0x00, 0x20}, {0x20, 0x0e, 0x00}, {0xf4, 0x13},
		{}, {0xff}, {0xff, 0xff, 0xff, 0xff},
	}
	for n := 0; n <= 6; n++ {
		for i := 0; i < 6; i++ {
			buf := make([]byte, n)
			r.Read(buf)
			buffers = append(buffers, buf)
		}
	}
	var out []bitutilsCase
	for _, data := range buffers {
		c := bitutilsCase{DataHex: hex.EncodeToString(data)}
		for start := uint(0); start <= uint(len(data))*8+16; start++ {
			c.Byte4 = append(c.Byte4, numResult(func() (uint64, error) {
				v, err := bitutils.ParseByte4(data, start)
				return uint64(v), err
			}))
			c.Byte8 = append(c.Byte8, numResult(func() (uint64, error) {
				v, err := bitutils.ParseByte8(data, start)
				return uint64(v), err
			}))
			c.UInt12 = append(c.UInt12, numResult(func() (uint64, error) {
				v, err := bitutils.ParseUInt12(data, start)
				return uint64(v), err
			}))
			c.UInt16 = append(c.UInt16, numResult(func() (uint64, error) {
				v, err := bitutils.ParseUInt16(data, start)
				return uint64(v), err
			}))
		}
		out = append(out, c)
	}
	return out
}

// numResult renders "v:<n>", "e:<error text>" or "p:<panic text>".
func numResult(f func() (uint64, error)) string {
	var v uint64
	var err error
	if msg, panicked := catch(func() { v, err = f() }); panicked {
		return "p:" + msg
	}
	if err != nil {
		return "e:" + err.Error()
	}
	return "v:" + strconv.FormatUint(v, 10)
}
