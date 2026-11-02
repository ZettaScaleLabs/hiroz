package hiroz

import (
	"fmt"
	"sort"
	"sync"
	"time"
)

// CachedSample associates a copied sample with a timestamp selected by the caller.
type CachedSample struct {
	Timestamp time.Time
	Sample    Sample
}

// SampleCache keeps at most capacity samples sorted by timestamp. Older samples
// are evicted first, including late arrivals older than the retained window.
// Queries return deep copies and do not expose mutable cache storage.
type SampleCache struct {
	mu       sync.RWMutex
	capacity int
	samples  []CachedSample
}

func NewSampleCache(capacity int) (*SampleCache, error) {
	if capacity < 1 {
		return nil, fmt.Errorf("capacity must be positive")
	}
	return &SampleCache{capacity: capacity}, nil
}
func cloneSample(s Sample) Sample {
	s.Data = append([]byte(nil), s.Data...)
	if s.Attachment != nil {
		s.Attachment = append([]byte{}, s.Attachment...)
	}
	if s.SequenceNumber != nil {
		n := *s.SequenceNumber
		s.SequenceNumber = &n
	}
	if s.SourceTimestamp != nil {
		n := *s.SourceTimestamp
		s.SourceTimestamp = &n
	}
	if s.PublisherGID != nil {
		n := *s.PublisherGID
		s.PublisherGID = &n
	}
	return s
}
func cloneCached(s CachedSample) CachedSample { s.Sample = cloneSample(s.Sample); return s }
func (c *SampleCache) Add(timestamp time.Time, sample Sample) {
	c.mu.Lock()
	defer c.mu.Unlock()
	i := sort.Search(len(c.samples), func(i int) bool { return c.samples[i].Timestamp.After(timestamp) })
	c.samples = append(c.samples, CachedSample{})
	copy(c.samples[i+1:], c.samples[i:])
	c.samples[i] = CachedSample{timestamp, cloneSample(sample)}
	if len(c.samples) > c.capacity {
		c.samples[0] = CachedSample{}
		c.samples = c.samples[1:]
	}
}

// AddSample indexes ROS source time if present, otherwise callback receipt time.
func (c *SampleCache) AddSample(sample Sample) {
	timestamp := sample.ReceivedAt
	if sample.SourceTimestamp != nil {
		timestamp = time.Unix(0, *sample.SourceTimestamp)
	}
	c.Add(timestamp, sample)
}
func (c *SampleCache) Len() int { c.mu.RLock(); defer c.mu.RUnlock(); return len(c.samples) }
func (c *SampleCache) Clear()   { c.mu.Lock(); c.samples = nil; c.mu.Unlock() }

// Between returns samples in the inclusive interval [start,end].
func (c *SampleCache) Between(start, end time.Time) []CachedSample {
	c.mu.RLock()
	defer c.mu.RUnlock()
	result := []CachedSample{}
	for _, s := range c.samples {
		if !s.Timestamp.Before(start) && !s.Timestamp.After(end) {
			result = append(result, cloneCached(s))
		}
	}
	return result
}
func (c *SampleCache) BeforeOrAt(timestamp time.Time) (CachedSample, bool) {
	c.mu.RLock()
	defer c.mu.RUnlock()
	i := sort.Search(len(c.samples), func(i int) bool { return c.samples[i].Timestamp.After(timestamp) })
	if i == 0 {
		return CachedSample{}, false
	}
	return cloneCached(c.samples[i-1]), true
}
func (c *SampleCache) AfterOrAt(timestamp time.Time) (CachedSample, bool) {
	c.mu.RLock()
	defer c.mu.RUnlock()
	i := sort.Search(len(c.samples), func(i int) bool { return !c.samples[i].Timestamp.Before(timestamp) })
	if i == len(c.samples) {
		return CachedSample{}, false
	}
	return cloneCached(c.samples[i]), true
}

// BuildWithCache subscribes with a bounded timestamp cache and returns both.
func (b *SubscriberBuilder) BuildWithCache(msg Message, capacity int) (*Subscriber, *SampleCache, error) {
	c, err := NewSampleCache(capacity)
	if err != nil {
		return nil, nil, err
	}
	s, err := b.BuildWithSampleCallback(msg, c.AddSample)
	return s, c, err
}
