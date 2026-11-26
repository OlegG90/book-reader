// @vitest-environment jsdom
import { describe, expect, it } from 'vitest'
import { neighbour, onSwipe, ownsSideways, swipeDirection } from './swipe'

const at = (x: number, y = 300, time = 0) => ({ x, y, time })

describe('swipeDirection', () => {
  it('reads a quick sideways swipe', () => {
    expect(swipeDirection(at(300), at(150, 310, 200), 400)).toBe('next')
    expect(swipeDirection(at(100), at(250, 290, 200), 400)).toBe('prev')
  })

  it('ignores short, slow, slanted and edge touches', () => {
    expect(swipeDirection(at(300), at(260, 300, 100), 400)).toBeNull()
    expect(swipeDirection(at(300), at(100, 300, 900), 400)).toBeNull()
    expect(swipeDirection(at(300), at(200, 400, 100), 400)).toBeNull()
    // The system's back gesture starts at the edges.
    expect(swipeDirection(at(10), at(200, 300, 100), 400)).toBeNull()
    expect(swipeDirection(at(390), at(200, 300, 100), 400)).toBeNull()
  })
})

describe('ownsSideways', () => {
  it('leaves fields and sliders their own moves', () => {
    document.body.innerHTML = '<div><input type="range" id="slider"><p id="text">x</p></div>'
    expect(ownsSideways(document.getElementById('slider'))).toBe(true)
    expect(ownsSideways(document.getElementById('text'))).toBe(false)
  })
})

describe('onSwipe', () => {
  const touch = (type: string, x: number, timeStamp: number) =>
    Object.defineProperties(new Event(type, { bubbles: true }), {
      pointerType: { value: 'touch' },
      clientX: { value: x },
      clientY: { value: 300 },
      timeStamp: { value: timeStamp },
    })

  it('reports one swipe per gesture, and none while disabled', () => {
    document.body.innerHTML = '<div id="outer"><p id="text">x</p></div>'
    const outer = document.getElementById('outer')!
    const seen: string[] = []
    let enabled = true
    onSwipe(outer, () => enabled, direction => seen.push(direction))
    const text = document.getElementById('text')!
    text.dispatchEvent(touch('pointerdown', 300, 0))
    text.dispatchEvent(touch('pointerup', 150, 100))
    enabled = false
    text.dispatchEvent(touch('pointerdown', 300, 200))
    text.dispatchEvent(touch('pointerup', 150, 300))
    expect(seen).toEqual(['next'])
  })
})

describe('neighbour', () => {
  it('steps along the order without wrapping', () => {
    const order = ['recent', 'library', 'series'] as const
    expect(neighbour(order, 'library', 'next')).toBe('series')
    expect(neighbour(order, 'library', 'prev')).toBe('recent')
    expect(neighbour(order, 'series', 'next')).toBeNull()
    expect(neighbour(order, 'recent', 'prev')).toBeNull()
  })
})
