/*
 * Copyright (c) 2019-present, Trail of Bits, Inc.
 * All rights reserved.
 *
 * This source code is licensed in accordance with the terms specified in
 * the LICENSE file found in the root directory of this source tree.
 */

#pragma once

#include <llvm/IR/Attributes.h>
#include <remill/BC/Util.h>

#include <optional>
#include <vector>

#include "Arch.h"

namespace remill {

class Arch;

}  // namespace remill
namespace anvill {

class CallingConvention;

struct AllocationConfig {
  bool can_pack_multiple_values_together{false};
  llvm::Type *(*type_splitter)(llvm::Type *) = nullptr;
};

// Captures the state of allocation for registers, including which registers
// are taken and the available space left in each register. The AllocationState
// needs a reference to the working architecture so that it can lookup
// registers.
struct AllocationState {
 public:
  ~AllocationState(void);

  AllocationState(const std::vector<RegisterConstraint> &_constraints,
                  const remill::Arch *_arch, const CallingConvention *_conv);

  SizeAndType AssignSizeAndType(llvm::Type &type);

  std::optional<std::vector<ValueDecl>> TryRegisterAllocate(llvm::Type &type);

  std::optional<std::vector<anvill::ValueDecl>>
  TryCompositeRegisterAllocate(llvm::Type &type);

  std::optional<std::vector<ValueDecl>>
  TryBasicRegisterAllocate(llvm::Type &type, std::optional<SizeAndType> hint);

  std::optional<std::vector<anvill::ValueDecl>>
  TryVectorRegisterAllocate(llvm::FixedVectorType &type);

  bool IsFilled(size_t i);

  uint64_t RemainingSpace(size_t i);

  std::optional<std::vector<anvill::ValueDecl>>
  ProcessIntVecX86_64SysV(llvm::Type *elem_type, unsigned int vec_size,
                          unsigned int bit_width);

  llvm::Error CoalescePacking(const std::vector<anvill::ValueDecl> &vector,
                              std::vector<anvill::ValueDecl> &packed_values);

  const std::vector<RegisterConstraint> &constraints;
  const remill::Arch *arch;
  std::vector<bool> reserved;
  std::vector<uint64_t> fill;
  const CallingConvention *conv;
  AllocationConfig config;
  const SizeConstraint ptr_size_constraint;
};

}  // namespace anvill
