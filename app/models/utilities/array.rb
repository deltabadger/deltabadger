module Utilities
  module Array
    def self.sort_arrays_by_first_array(array, *others)
      raise ArgumentError, 'All arrays must have the same length' unless others.all? { |arr| arr.length == array.length }

      combined = array.zip(*others)
      sorted = combined.sort_by { |tuple| tuple[0] }
      sorted.transpose
    end
  end
end
