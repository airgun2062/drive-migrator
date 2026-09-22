import { useCallback, useEffect, useRef, useState, type ReactNode } from 'react'

// Fixed-row-height windowed list: renders only what's on screen (plus a
// small overscan buffer) regardless of how many items are passed in, so a
// duplicate-group or plan-entry list with tens of thousands of rows
// (SPEC.md section 9) never has to mount tens of thousands of DOM nodes.
interface VirtualListProps<T> {
  items: T[]
  itemHeight: number
  renderItem: (item: T, index: number) => ReactNode
  overscan?: number
  emptyMessage?: string
  className?: string
}

export function VirtualList<T>({
  items,
  itemHeight,
  renderItem,
  overscan = 6,
  emptyMessage,
  className,
}: VirtualListProps<T>) {
  const containerRef = useRef<HTMLDivElement>(null)
  const [scrollTop, setScrollTop] = useState(0)
  const [viewportHeight, setViewportHeight] = useState(0)

  const onScroll = useCallback(() => {
    if (containerRef.current) setScrollTop(containerRef.current.scrollTop)
  }, [])

  useEffect(() => {
    const el = containerRef.current
    if (!el) return
    const observer = new ResizeObserver(() => setViewportHeight(el.clientHeight))
    observer.observe(el)
    setViewportHeight(el.clientHeight)
    return () => observer.disconnect()
  }, [])

  if (items.length === 0) {
    return (
      <div ref={containerRef} className={className}>
        {emptyMessage ? <p className="empty-message">{emptyMessage}</p> : null}
      </div>
    )
  }

  const totalHeight = items.length * itemHeight
  const firstVisible = Math.max(0, Math.floor(scrollTop / itemHeight) - overscan)
  const visibleCount = Math.ceil(viewportHeight / itemHeight) + overscan * 2
  const lastVisible = Math.min(items.length, firstVisible + visibleCount)
  const visible = items.slice(firstVisible, lastVisible)

  return (
    <div ref={containerRef} className={className} onScroll={onScroll}>
      <div style={{ height: totalHeight, position: 'relative' }}>
        {visible.map((item, i) => {
          const index = firstVisible + i
          return (
            <div
              key={index}
              style={{
                position: 'absolute',
                top: index * itemHeight,
                left: 0,
                right: 0,
                height: itemHeight,
              }}
            >
              {renderItem(item, index)}
            </div>
          )
        })}
      </div>
    </div>
  )
}
