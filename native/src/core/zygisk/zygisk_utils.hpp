#pragma once

#include <string>
#include <vector>
#include <bitset>
#include <cstdint>
#include <string_view>

static inline bool str_ends(const std::string_view &str, const std::string_view &suffix) {
    return str.size() >= suffix.size() &&
           str.compare(str.size() - suffix.size(), suffix.size(), suffix) == 0;
}

template<size_t BlockBits = 64>
class dynamic_bitset {
    std::vector<std::bitset<BlockBits>> blocks_;
    size_t size_ = 0;
public:
    using slot_type = unsigned long;

    dynamic_bitset() = default;
    dynamic_bitset(size_t n) : size_(n), blocks_((n + BlockBits - 1) / BlockBits) {}

    void resize(size_t n) {
        blocks_.resize((n + BlockBits - 1) / BlockBits);
        size_ = n;
    }

    void set(size_t pos, bool val = true) {
        if (pos >= size_) return;
        blocks_[pos / BlockBits].set(pos % BlockBits, val);
    }

    bool test(size_t pos) const {
        if (pos >= size_) return false;
        return blocks_[pos / BlockBits].test(pos % BlockBits);
    }

    size_t size() const { return size_; }

    void emplace_back(slot_type l) {
        blocks_.emplace_back(l);
    }

    bool operator[](size_t pos) const { return test(pos); }

    // Non-const version returns a reference wrapper for write access: bits[pos] = val
    using reference = typename std::bitset<BlockBits>::reference;
    reference operator[](size_t pos) {
        return blocks_[pos / BlockBits][pos % BlockBits];
    }

    bool any() const {
        for (auto &b : blocks_) if (b.any()) return true;
        return false;
    }

    size_t slots() const { return blocks_.size(); }

    slot_type get_slot(size_t i) const {
        return i < blocks_.size() ? blocks_[i].to_ulong() : 0;
    }
};
