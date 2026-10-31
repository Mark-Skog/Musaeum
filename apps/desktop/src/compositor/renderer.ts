/**
 * The WebGL2 side of the compositor: one texture array holding every resident tile, and
 * one instanced draw call per frame. WebGL2 is the baseline because this is the renderer
 * Windows and macOS both have on an `OffscreenCanvas`; nothing here needs compute.
 */

import { TILE_SIZE } from '../protocol';
import type { QuadSink } from './frame-planner';

const VERTEX_SHADER = `#version 300 es
layout(location = 0) in vec4 a_destination;
layout(location = 1) in vec4 a_source;
layout(location = 2) in float a_layer;
uniform vec2 u_viewport;
out vec2 v_uv;
flat out float v_layer;
void main() {
  vec2 corner = vec2(float(gl_VertexID & 1), float((gl_VertexID >> 1) & 1));
  vec2 position = a_destination.xy + corner * a_destination.zw;
  v_uv = mix(a_source.xy, a_source.zw, corner);
  v_layer = a_layer;
  vec2 clip = position / u_viewport * 2.0 - 1.0;
  gl_Position = vec4(clip.x, -clip.y, 0.0, 1.0);
}`;

const FRAGMENT_SHADER = `#version 300 es
precision highp float;
precision highp sampler2DArray;
in vec2 v_uv;
flat in float v_layer;
uniform sampler2DArray u_tiles;
uniform vec3 u_paper;
out vec4 o_color;
void main() {
  // A negative layer marks a plate. Tiles are opaque, so alpha never matters here.
  o_color = v_layer < 0.0
    ? vec4(u_paper, 1.0)
    : vec4(texture(u_tiles, vec3(v_uv, v_layer)).rgb, 1.0);
}`;

const FLOATS_PER_QUAD = 9;
const MAX_QUADS = 8192;

/** Matches the white the render worker fills tiles with, so plates never flash. */
const PAPER_COLOR: readonly [number, number, number] = [1, 1, 1];
const BACKDROP_COLOR: readonly [number, number, number] = [0.165, 0.165, 0.173];

export class TileRenderer implements QuadSink {
  private readonly gl: WebGL2RenderingContext;
  private readonly quads = new Float32Array(MAX_QUADS * FLOATS_PER_QUAD);
  private quadCount = 0;
  private viewportUniform: WebGLUniformLocation | null = null;
  /** Quads dropped because the frame exceeded `MAX_QUADS`. Should stay at zero. */
  droppedQuads = 0;
  readonly gpuName: string;

  constructor(
    private readonly canvas: OffscreenCanvas,
    readonly layers: number,
    onContextLost: () => void,
    onContextRestored: () => void,
  ) {
    const gl = canvas.getContext('webgl2', {
      alpha: false,
      antialias: false,
      depth: false,
      stencil: false,
      powerPreference: 'high-performance',
      preserveDrawingBuffer: false,
    });
    if (gl === null) {
      throw new Error('WebGL2 is not available on OffscreenCanvas');
    }
    this.gl = gl;

    const maxLayers = gl.getParameter(gl.MAX_ARRAY_TEXTURE_LAYERS) as number;
    if (layers > maxLayers) {
      throw new Error(`the GPU allows ${maxLayers} texture layers, ${layers} were requested`);
    }
    const debug = gl.getExtension('WEBGL_debug_renderer_info');
    this.gpuName = debug
      ? (gl.getParameter(debug.UNMASKED_RENDERER_WEBGL) as string)
      : (gl.getParameter(gl.RENDERER) as string);

    canvas.addEventListener('webglcontextlost', (event) => {
      event.preventDefault();
      onContextLost();
    });
    canvas.addEventListener('webglcontextrestored', () => {
      this.createResources();
      onContextRestored();
    });
    this.createResources();
  }

  /** Everything here is lost with the context and created again after a restore. */
  private createResources(): void {
    const { gl } = this;
    const program = linkProgram(gl);
    gl.useProgram(program);
    this.viewportUniform = gl.getUniformLocation(program, 'u_viewport');
    gl.uniform1i(gl.getUniformLocation(program, 'u_tiles'), 0);
    gl.uniform3fv(gl.getUniformLocation(program, 'u_paper'), PAPER_COLOR);

    const texture = gl.createTexture();
    gl.activeTexture(gl.TEXTURE0);
    gl.bindTexture(gl.TEXTURE_2D_ARRAY, texture);
    gl.texStorage3D(gl.TEXTURE_2D_ARRAY, 1, gl.RGBA8, TILE_SIZE, TILE_SIZE, this.layers);
    gl.texParameteri(gl.TEXTURE_2D_ARRAY, gl.TEXTURE_MIN_FILTER, gl.LINEAR);
    gl.texParameteri(gl.TEXTURE_2D_ARRAY, gl.TEXTURE_MAG_FILTER, gl.LINEAR);
    gl.texParameteri(gl.TEXTURE_2D_ARRAY, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
    gl.texParameteri(gl.TEXTURE_2D_ARRAY, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
    gl.pixelStorei(gl.UNPACK_ALIGNMENT, 4);

    const buffer = gl.createBuffer();
    gl.bindVertexArray(gl.createVertexArray());
    gl.bindBuffer(gl.ARRAY_BUFFER, buffer);
    gl.bufferData(gl.ARRAY_BUFFER, this.quads.byteLength, gl.DYNAMIC_DRAW);
    const stride = FLOATS_PER_QUAD * 4;
    gl.enableVertexAttribArray(0);
    gl.vertexAttribPointer(0, 4, gl.FLOAT, false, stride, 0);
    gl.vertexAttribDivisor(0, 1);
    gl.enableVertexAttribArray(1);
    gl.vertexAttribPointer(1, 4, gl.FLOAT, false, stride, 16);
    gl.vertexAttribDivisor(1, 1);
    gl.enableVertexAttribArray(2);
    gl.vertexAttribPointer(2, 1, gl.FLOAT, false, stride, 32);
    gl.vertexAttribDivisor(2, 1);

    gl.disable(gl.DEPTH_TEST);
    gl.disable(gl.BLEND);
    gl.clearColor(BACKDROP_COLOR[0], BACKDROP_COLOR[1], BACKDROP_COLOR[2], 1);
  }

  resize(width: number, height: number): void {
    this.canvas.width = width;
    this.canvas.height = height;
    this.gl.viewport(0, 0, width, height);
  }

  /** Copies one tile to the GPU. The array accepts both normal and shared buffers. */
  upload(layer: number, pixels: Uint8Array): void {
    const { gl } = this;
    gl.texSubImage3D(
      gl.TEXTURE_2D_ARRAY,
      0,
      0,
      0,
      layer,
      TILE_SIZE,
      TILE_SIZE,
      1,
      gl.RGBA,
      gl.UNSIGNED_BYTE,
      pixels as Uint8Array<ArrayBuffer>,
    );
  }

  plate(x: number, y: number, width: number, height: number): void {
    this.push(x, y, width, height, 0, 0, 0, 0, -1);
  }

  tile(
    x: number,
    y: number,
    width: number,
    height: number,
    u0: number,
    v0: number,
    u1: number,
    v1: number,
    layer: number,
  ): void {
    this.push(x, y, width, height, u0, v0, u1, v1, layer);
  }

  private push(
    x: number,
    y: number,
    width: number,
    height: number,
    u0: number,
    v0: number,
    u1: number,
    v1: number,
    layer: number,
  ): void {
    if (this.quadCount >= MAX_QUADS) {
      this.droppedQuads++;
      return;
    }
    const at = this.quadCount++ * FLOATS_PER_QUAD;
    const quads = this.quads;
    quads[at] = x;
    quads[at + 1] = y;
    quads[at + 2] = width;
    quads[at + 3] = height;
    quads[at + 4] = u0;
    quads[at + 5] = v0;
    quads[at + 6] = u1;
    quads[at + 7] = v1;
    quads[at + 8] = layer;
  }

  /** Draws everything pushed since the last call, in the order it was pushed. */
  flush(): void {
    const { gl } = this;
    gl.clear(gl.COLOR_BUFFER_BIT);
    if (this.quadCount > 0) {
      gl.uniform2f(this.viewportUniform, this.canvas.width, this.canvas.height);
      gl.bufferSubData(gl.ARRAY_BUFFER, 0, this.quads, 0, this.quadCount * FLOATS_PER_QUAD);
      gl.drawArraysInstanced(gl.TRIANGLE_STRIP, 0, 4, this.quadCount);
    }
    this.quadCount = 0;
  }
}

function compileShader(gl: WebGL2RenderingContext, type: GLenum, source: string): WebGLShader {
  const shader = gl.createShader(type);
  if (shader === null) {
    throw new Error('could not create a shader');
  }
  gl.shaderSource(shader, source);
  gl.compileShader(shader);
  if (!gl.getShaderParameter(shader, gl.COMPILE_STATUS)) {
    throw new Error(`shader failed to compile: ${gl.getShaderInfoLog(shader)}`);
  }
  return shader;
}

function linkProgram(gl: WebGL2RenderingContext): WebGLProgram {
  const program = gl.createProgram();
  gl.attachShader(program, compileShader(gl, gl.VERTEX_SHADER, VERTEX_SHADER));
  gl.attachShader(program, compileShader(gl, gl.FRAGMENT_SHADER, FRAGMENT_SHADER));
  gl.linkProgram(program);
  if (!gl.getProgramParameter(program, gl.LINK_STATUS)) {
    throw new Error(`shader program failed to link: ${gl.getProgramInfoLog(program)}`);
  }
  return program;
}
